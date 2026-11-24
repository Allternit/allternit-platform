//! Whole-disk file roots on servers (Computers console step 2.11,
//! surfaces/docs/dev/computers-console-contract.mdx "Whole-disk files").
//!
//! A per-computer setting, OFF by default. Off, the console Files tab sees only
//! the Files drive folder, as before. On, the owner may browse the whole disk
//! of a server they own (a paired machine or an Allternit Cloud computer; never
//! a shared row, never a Desktop).
//!
//! * `GET /api/v1/computers/:id/file-roots` -> `{ id, wholeDisk }`
//! * `PATCH /api/v1/computers/:id/file-roots` `{ wholeDisk: bool, confirm? }`
//!   -> `{ id, wholeDisk }`. Owner only; turning it ON needs `confirm: true`.
//! * Console rows carry the same `wholeDisk` object.
//!
//! `wholeDisk` = `{ enabled, available, reason?, roots }`; `roots` is filled
//! only while enabled (`/` on Linux and macOS, drive letters on Windows).
//!
//! **Enforcement (fail closed).** Every relayed request to a file route that
//! names a path outside the Files drive (an absolute path outside the home
//! folder, a `..` step, or a `kind: "disk"` root) is refused with 403
//! `whole_disk_off` unless the setting is on for that exact runtime
//! ([`refuse_outside_drive`], called by `runtime_relay`). The owner's browser
//! proxy also sends the trusted header [`WHOLE_DISK_HEADER`] (`on`) while it is
//! on; no client can send that header (the relay's allow-list drops it), so the
//! runtime treats its absence as OFF. Nothing here talks to the computer, so
//! nothing here wakes it. Setting changes write `audit_log`
//! (`computer.whole_disk.on` / `.off`), as unpair does; file changes on the
//! computer keep the runtime's own change records.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::routes::computers_console::{find_machine, ConsoleComputer, GROUP_SHARED};
use crate::routes::paired_computers::signed_in_caller;
use crate::services::audit::{write_audit_log, AuditEvent};
use crate::{ApiError, ApiState};

/// Trusted relay header: `on` while whole-disk browsing is on for the runtime.
pub const WHOLE_DISK_HEADER: &str = "x-allternit-whole-disk";
pub const OFF_CODE: &str = "whole_disk_off";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route(
        "/api/v1/computers/:id/file-roots",
        get(get_roots).patch(patch_roots),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskRoot {
    pub id: String,
    pub path: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WholeDisk {
    pub enabled: bool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    pub roots: Vec<DiskRoot>,
}

/// One stored setting row (migration 160).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Setting {
    pub machine_key: String,
    pub enabled: bool,
    pub runtime_ids: Vec<String>,
}

/// Why a row cannot have whole-disk roots, if it cannot.
pub fn unavailable_reason(row: &ConsoleComputer) -> Option<&'static str> {
    if row.group == GROUP_SHARED {
        Some("not_owner")
    } else if row.kind == "desktop" {
        Some("not_a_server")
    } else if row.runtime_id.is_none() {
        Some("no_runtime")
    } else {
        None
    }
}

/// `/` on Linux and macOS; on Windows every drive letter the newest host
/// sample lists (`lastSample.disk[].mount`), else `C:\`.
pub fn disk_roots(os_family: &str, last_sample: Option<&Value>) -> Vec<DiskRoot> {
    if os_family != "windows" {
        return vec![DiskRoot {
            id: "disk:/".into(),
            path: "/".into(),
            label: "Whole disk".into(),
        }];
    }
    let mut letters: Vec<char> = last_sample
        .and_then(|s| s.get("disk"))
        .and_then(Value::as_array)
        .map(|disks| {
            disks
                .iter()
                .filter_map(|d| d.get("mount").and_then(Value::as_str))
                .filter_map(|m| {
                    let mut c = m.chars();
                    let letter = c.next()?.to_ascii_uppercase();
                    (letter.is_ascii_alphabetic() && c.next() == Some(':')).then_some(letter)
                })
                .collect()
        })
        .unwrap_or_default();
    letters.sort_unstable();
    letters.dedup();
    if letters.is_empty() {
        letters.push('C');
    }
    letters
        .into_iter()
        .map(|l| DiskRoot {
            id: format!("disk:{l}:"),
            path: format!("{l}:\\"),
            label: format!("Drive {l}:"),
        })
        .collect()
}

/// The `wholeDisk` object for a console row. On only while the stored setting
/// is on AND covers the runtime the relay would call.
pub fn whole_disk_for(row: &ConsoleComputer, setting: Option<&Setting>) -> WholeDisk {
    let reason = unavailable_reason(row);
    let enabled = reason.is_none()
        && setting.is_some_and(|s| {
            s.enabled
                && row
                    .runtime_id
                    .as_ref()
                    .is_some_and(|r| s.runtime_ids.contains(r))
        });
    WholeDisk {
        enabled,
        available: reason.is_none(),
        reason,
        roots: if enabled {
            disk_roots(row.os_family, row.last_sample.as_ref())
        } else {
            Vec::new()
        },
    }
}

/// Fill `wholeDisk` on every row (console list). A missing table (migration
/// 160 not applied yet) reads as OFF everywhere.
pub async fn apply_whole_disk(db: &sqlx::PgPool, user_id: &str, rows: &mut [ConsoleComputer]) {
    let settings = load_settings(db, user_id).await.unwrap_or_else(|e| {
        tracing::warn!("whole-disk settings unreadable, treating as off: {e}");
        Vec::new()
    });
    for row in rows.iter_mut() {
        let setting = settings.iter().find(|s| s.machine_key == row.machine_key);
        row.whole_disk = Some(whole_disk_for(row, setting));
    }
}

async fn load_settings(db: &sqlx::PgPool, user_id: &str) -> Result<Vec<Setting>, sqlx::Error> {
    sqlx::query_as(
        "SELECT machine_key, enabled, runtime_ids FROM computer_whole_disk WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

/// Is whole-disk browsing on for this exact runtime of this owner? Any error
/// (including a missing table) is OFF.
pub async fn whole_disk_on(db: &sqlx::PgPool, user_id: &str, runtime_id: &str) -> bool {
    let found: Result<Option<bool>, sqlx::Error> = sqlx::query_scalar(
        "SELECT true FROM computer_whole_disk WHERE user_id = $1 AND enabled AND $2 = ANY(runtime_ids) LIMIT 1",
    )
    .bind(user_id)
    .bind(runtime_id)
    .fetch_optional(db)
    .await;
    match found {
        Ok(v) => v.unwrap_or(false),
        Err(e) => {
            tracing::warn!("whole-disk check failed, treating as off: {e}");
            false
        }
    }
}

// ------------------------------------------------------------ path checks ----

/// Runtime file routes: legacy `/api/v1/files/*`, Files drive v2
/// `/api/v1/files/v2/*`, and `/api/v1/computers/:id/files/{upload,download}`.
pub fn is_file_route(path: &str) -> bool {
    let bare = path.split(['?', '#']).next().unwrap_or_default();
    bare == "/api/v1/files"
        || bare.starts_with("/api/v1/files/")
        || (bare.starts_with("/api/v1/computers/") && bare.contains("/files/"))
}

/// Path-like keys in a query string or a JSON body.
const PATH_KEYS: &[&str] = &[
    "path",
    "paths",
    "local_path",
    "localPath",
    "src",
    "dest",
    "from",
    "to",
    "source",
    "destination",
    "dir",
    "directory",
    "target",
];

/// Largest JSON body that is read for paths. File content is never parsed.
const MAX_SCAN_BODY: usize = 64 * 1024;

/// True when `p` names something outside the home folder (and so outside the
/// Files drive): an absolute path not under a user folder, `~other`, a UNC
/// path, or any `..` step. Relative and `~/` paths are inside (the runtime
/// joins them to the home folder and scopes them itself).
pub fn outside_home(p: &str) -> bool {
    let p = p.trim();
    if p.is_empty() {
        return false;
    }
    let norm = p.replace('\\', "/");
    if norm.split('/').any(|seg| seg == "..") {
        return true;
    }
    if norm == "~" || norm.starts_with("~/") {
        return false;
    }
    if norm.starts_with('~') || norm.starts_with("//") {
        return true;
    }
    let bytes = norm.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if drive {
        // X:\Users\<name>\...
        let rest: Vec<&str> = norm[2..].split('/').filter(|s| !s.is_empty()).collect();
        return !(rest.len() >= 2 && rest[0].eq_ignore_ascii_case("users"));
    }
    if !norm.starts_with('/') {
        return false;
    }
    let segs: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        ["root", ..] => false,
        ["home" | "Users", _user, ..] => false,
        ["var", "home", _user, ..] => false,
        _ => true,
    }
}

fn query_paths(path: &str) -> Vec<String> {
    let Some((_, query)) = path.split_once('?') else {
        return Vec::new();
    };
    url::form_urlencoded::parse(query.split('#').next().unwrap_or_default().as_bytes())
        .filter(|(k, _)| PATH_KEYS.contains(&k.as_ref()))
        .map(|(_, v)| v.into_owned())
        .collect()
}

/// Does this file request reach outside the Files drive?
pub fn reaches_outside_drive(method: &str, path: &str, body: &[u8]) -> bool {
    if !is_file_route(path) {
        return false;
    }
    if query_paths(path).iter().any(|p| outside_home(p)) {
        return true;
    }
    let bare = path.split('?').next().unwrap_or_default();
    let scan = !method.eq_ignore_ascii_case("GET")
        && body.len() <= MAX_SCAN_BODY
        && !bare.ends_with("/content");
    if !scan {
        return false;
    }
    let Ok(Value::Object(obj)) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    if obj.get("kind").and_then(Value::as_str) == Some("disk") {
        return true;
    }
    PATH_KEYS
        .iter()
        .filter_map(|k| obj.get(*k))
        .any(|v| match v {
            Value::String(s) => outside_home(s),
            Value::Array(items) => items.iter().filter_map(Value::as_str).any(outside_home),
            _ => false,
        })
}

pub fn off_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": OFF_CODE,
            "message": "This folder is outside the Files drive. Turn on whole-disk browsing for this computer in its Files settings first.",
        })),
    )
        .into_response()
}

/// The relay gate: `Some(403 whole_disk_off)` when the request reaches outside
/// the Files drive and the setting is off for this runtime.
pub async fn refuse_outside_drive(
    db: &sqlx::PgPool,
    user_id: &str,
    runtime_id: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> Option<Response> {
    if !reaches_outside_drive(method, path, body) {
        return None;
    }
    if whole_disk_on(db, user_id, runtime_id).await {
        return None;
    }
    tracing::info!(
        runtime_id,
        "relay: file path outside the Files drive refused (whole-disk off)"
    );
    Some(off_response())
}

// ----------------------------------------------------------------- routes ----

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn db_fail(e: sqlx::Error) -> Response {
    tracing::warn!("computer file roots: {e}");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db_error",
        "Something went wrong. Try again in a moment.",
    )
}

async fn machine(
    state: &ApiState,
    headers: &HeaderMap,
    id: &str,
) -> Result<Result<(String, ConsoleComputer), Response>, ApiError> {
    let caller = signed_in_caller(state, headers).await?;
    let row = match find_machine(
        &state.db,
        &caller.user_id,
        caller.organization_id.as_deref(),
        id,
    )
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Ok(Err(fail(
                StatusCode::NOT_FOUND,
                "not_found",
                "That computer isn't in your Computers list.",
            )))
        }
        Err(e) => return Ok(Err(db_fail(e))),
    };
    Ok(Ok((caller.user_id, row)))
}

async fn get_roots(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (_, row) = match machine(&state, &headers, &id).await? {
        Ok(found) => found,
        Err(response) => return Ok(response),
    };
    let wd = row
        .whole_disk
        .clone()
        .unwrap_or_else(|| whole_disk_for(&row, None));
    Ok(Json(json!({ "id": row.id, "wholeDisk": wd })).into_response())
}

async fn patch_roots(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let body = body.map(|Json(b)| b).unwrap_or(Value::Null);
    let Some(on) = body.get("wholeDisk").and_then(Value::as_bool) else {
        return Ok(fail(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "Send wholeDisk: true or false.",
        ));
    };
    let (user_id, row) = match machine(&state, &headers, &id).await? {
        Ok(found) => found,
        Err(response) => return Ok(response),
    };
    match unavailable_reason(&row) {
        Some("not_owner") => {
            return Ok(fail(
                StatusCode::FORBIDDEN,
                "not_owner",
                "Someone else in your organization paired this computer. Only they can change it.",
            ))
        }
        Some("not_a_server") => {
            return Ok(fail(
                StatusCode::CONFLICT,
                "not_a_server",
                "Whole-disk browsing is for servers. This computer runs Allternit Desktop, which already reaches its own files.",
            ))
        }
        Some(_) if on => {
            return Ok(fail(
                StatusCode::CONFLICT,
                "no_runtime",
                "This computer has no Allternit runtime connected yet. Pair it with gizzi pair first.",
            ))
        }
        _ => {}
    }
    if on && body.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Ok(fail(
            StatusCode::BAD_REQUEST,
            "confirm_required",
            "Whole-disk browsing lets you open, change and delete any file on this computer. Send confirm: true to turn it on.",
        ));
    }
    let saved = sqlx::query(
        "INSERT INTO computer_whole_disk (user_id, machine_key, enabled, runtime_ids, enabled_at, updated_at)
         VALUES ($1, $2, $3, $4, CASE WHEN $3 THEN now() END, now())
         ON CONFLICT (user_id, machine_key) DO UPDATE SET
             enabled = EXCLUDED.enabled,
             runtime_ids = CASE WHEN EXCLUDED.enabled THEN EXCLUDED.runtime_ids ELSE computer_whole_disk.runtime_ids END,
             enabled_at = CASE WHEN EXCLUDED.enabled THEN now() ELSE computer_whole_disk.enabled_at END,
             updated_at = now()",
    )
    .bind(&user_id)
    .bind(&row.machine_key)
    .bind(on)
    .bind(&row.member_devices)
    .execute(&state.db)
    .await;
    if let Err(e) = saved {
        return Ok(db_fail(e));
    }
    write_audit_log(
        &state.db,
        AuditEvent {
            action: (if on {
                "computer.whole_disk.on"
            } else {
                "computer.whole_disk.off"
            })
            .into(),
            resource_type: "computer".into(),
            resource_id: Some(row.id.clone()),
            user_id: Some(user_id.clone()),
            user_email: None,
            details: Some(json!({ "runtimeIds": row.member_devices })),
            success: true,
        },
    )
    .await;
    let setting = Setting {
        machine_key: row.machine_key.clone(),
        enabled: on,
        runtime_ids: row.member_devices.clone(),
    };
    Ok(
        Json(json!({ "id": row.id, "wholeDisk": whole_disk_for(&row, Some(&setting)) }))
            .into_response(),
    )
}

#[cfg(test)]
#[path = "computer_file_roots_tests.rs"]
mod tests;
