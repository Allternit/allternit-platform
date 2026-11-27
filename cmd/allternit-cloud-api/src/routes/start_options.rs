//! Start anywhere (D7): the folder snapshot a computer pushes while online,
//! so a phone can offer its folders even when the computer is asleep.
//! Contract: `docs/contracts/session-start/presence.delta.json`.
//!
//! * `PUT /api/v1/runtime-devices/:id/start-options`: the computer's own
//!   device credential (`Authorization: Bearer allternit_runtime_…`), scoped
//!   to `:id`, so a device can only write its own snapshot. Body
//!   `{updatedAt, folders[<= 30], modes, defaultRuntime}`, at most 64 KB.
//!   Answers 204.
//! * `GET /api/v1/start-options`: signed-in owner →
//!   `{devices: [{runtimeId, name, kind, remoteStart, updatedAt, folders,
//!   modes, defaultRuntime}]}` for that owner's live (unrevoked) runtimes.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::Utc;
use serde_json::{json, Map, Value};

use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::{ApiError, ApiState};

const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_FOLDERS: usize = 30;
const MAX_TEXT: usize = 4096;
const MAX_ID: usize = 128;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/runtime-devices/:id/start-options", put(put_options))
        .route("/api/v1/start-options", get(list_options))
}

fn fail(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

fn text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= max)
        .map(str::to_string)
}

/// One folder with only the contract's fields; `None` when it has no path.
fn clean_folder(folder: &Value) -> Option<Value> {
    let path = text(&folder["path"], MAX_TEXT)?;
    let mut out = Map::new();
    out.insert("path".into(), json!(path));
    let name = text(&folder["name"], 256)
        .unwrap_or_else(|| path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(&path).to_string());
    out.insert("name".into(), json!(name));
    if let Some(at) = text(&folder["lastUsedAt"], 64) {
        out.insert("lastUsedAt".into(), json!(at));
    }
    let git = &folder["git"];
    if git.is_object() {
        let mut g = Map::new();
        for key in ["remote", "branch", "head"] {
            if let Some(v) = text(&git[key], MAX_TEXT) {
                g.insert(key.into(), json!(v));
            }
        }
        if let Some(dirty) = git["dirty"].as_bool() {
            g.insert("dirty".into(), json!(dirty));
        }
        if let Some(ahead) = git["ahead"].as_u64() {
            g.insert("ahead".into(), json!(ahead));
        }
        if !g.is_empty() {
            out.insert("git".into(), Value::Object(g));
        }
    }
    Some(Value::Object(out))
}

/// `modes: {<runtimeId>: {default, supported[]}}` with string mode ids only.
fn clean_modes(modes: &Value) -> Value {
    let Some(map) = modes.as_object() else {
        return json!({});
    };
    let out: Map<String, Value> = map
        .iter()
        .filter(|(runtime, _)| !runtime.is_empty() && runtime.len() <= 64)
        .take(32)
        .map(|(runtime, entry)| {
            let supported: Vec<Value> = entry["supported"]
                .as_array()
                .map(|list| list.iter().filter_map(|m| text(m, 32)).take(16).map(Value::String).collect())
                .unwrap_or_default();
            let default = text(&entry["default"], 32).map(Value::String).unwrap_or(Value::Null);
            (runtime.clone(), json!({ "default": default, "supported": supported }))
        })
        .collect();
    Value::Object(out)
}

/// The stored snapshot: contract fields only, at most [`MAX_FOLDERS`]
/// folders (the computer sends the most recently used first). `None` when
/// the body is not a JSON object.
pub(crate) fn clean_snapshot(body: &Value) -> Option<Value> {
    if !body.is_object() {
        return None;
    }
    let folders: Vec<Value> = body["folders"]
        .as_array()
        .map(|list| list.iter().filter_map(clean_folder).take(MAX_FOLDERS).collect())
        .unwrap_or_default();
    let updated_at = text(&body["updatedAt"], 64).unwrap_or_else(|| Utc::now().to_rfc3339());
    let default_runtime = text(&body["defaultRuntime"], 64).map(Value::String).unwrap_or(Value::Null);
    Some(json!({
        "updatedAt": updated_at,
        "folders": folders,
        "modes": clean_modes(&body["modes"]),
        "defaultRuntime": default_runtime,
    }))
}

async fn put_options(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    if body.len() > MAX_BODY_BYTES {
        return Ok(fail(StatusCode::PAYLOAD_TOO_LARGE, "too_large"));
    }
    if id.is_empty() || id.len() > MAX_ID {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    let token = device_token_from_headers(&headers)
        .ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    // Scoped to `:id`: another device's credential finds no row.
    let device = runtime_device_for_token(&state.db, token, Some(&id)).await?;
    let Ok(parsed) = serde_json::from_slice::<Value>(&body) else {
        return Ok(fail(StatusCode::BAD_REQUEST, "bad_body"));
    };
    let Some(snapshot) = clean_snapshot(&parsed) else {
        return Ok(fail(StatusCode::BAD_REQUEST, "bad_body"));
    };
    sqlx::query(
        "INSERT INTO runtime_start_options (runtime_id, user_id, options, updated_at) \
         VALUES ($1, $2, $3, CURRENT_TIMESTAMP) \
         ON CONFLICT (runtime_id) DO UPDATE SET user_id = EXCLUDED.user_id, \
         options = EXCLUDED.options, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(&device.id)
    .bind(&device.user_id)
    .bind(&snapshot)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn list_options(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let rows: Vec<(String, Option<String>, Option<String>, Value)> = sqlx::query_as(
        "SELECT o.runtime_id, d.name, d.runtime_type, o.options FROM runtime_start_options o \
         JOIN runtime_devices d ON d.id = o.runtime_id \
         WHERE o.user_id = $1 AND d.user_id = $1 AND d.revoked_at IS NULL \
         ORDER BY o.updated_at DESC",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let mut devices = Vec::with_capacity(rows.len());
    for (runtime_id, name, runtime_type, options) in rows {
        let remote_start = super::runtime_relay::runtime_remote_start(&runtime_id).await;
        devices.push(json!({
            "runtimeId": runtime_id,
            "name": name.unwrap_or_else(|| "Computer".into()),
            "kind": super::runtime_presence::presence_kind(runtime_type.as_deref().unwrap_or_default()),
            "remoteStart": remote_start,
            "updatedAt": options["updatedAt"],
            "folders": options.get("folders").cloned().unwrap_or_else(|| json!([])),
            "modes": options.get("modes").cloned().unwrap_or_else(|| json!({})),
            "defaultRuntime": options["defaultRuntime"],
        }));
    }
    Ok(Json(json!({ "devices": devices })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_keeps_contract_fields_and_caps_folders() {
        let folders: Vec<Value> = (0..40)
            .map(|i| json!({ "path": format!("/Users/dev/code/p{i}"), "name": format!("p{i}"), "secret": "x" }))
            .collect();
        let s = clean_snapshot(&json!({
            "updatedAt": "2026-11-27T15:45:00Z",
            "folders": folders,
            "modes": { "allternit": { "default": "default", "supported": ["auto", "default", "plan", "bypass", 7] } },
            "defaultRuntime": "allternit",
            "extra": true,
        }))
        .unwrap();
        assert_eq!(s["folders"].as_array().unwrap().len(), MAX_FOLDERS);
        assert!(s["folders"][0].get("secret").is_none());
        assert!(s.get("extra").is_none());
        assert_eq!(s["modes"]["allternit"]["supported"].as_array().unwrap().len(), 4);
        assert_eq!(s["defaultRuntime"], "allternit");
        assert_eq!(s["updatedAt"], "2026-11-27T15:45:00Z");
    }

    #[test]
    fn folder_needs_a_path_and_keeps_git() {
        let s = clean_snapshot(&json!({ "folders": [
            { "name": "no path" },
            { "path": "/Users/dev/code/allternit", "git": { "remote": "https://github.com/example-org/allternit.git", "branch": "main", "dirty": false, "ahead": 0 } },
            { "path": "/Users/dev/Documents/taxes-2026/" },
        ]}))
        .unwrap();
        let f = s["folders"].as_array().unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0]["name"], "allternit");
        assert_eq!(f[0]["git"]["branch"], "main");
        assert_eq!(f[0]["git"]["dirty"], false);
        assert_eq!(f[1]["name"], "taxes-2026");
        assert!(f[1].get("git").is_none());
        assert_eq!(s["modes"], json!({}));
        assert_eq!(s["defaultRuntime"], Value::Null);
    }

    #[test]
    fn non_object_body_is_refused() {
        assert!(clean_snapshot(&json!([])).is_none());
        assert!(clean_snapshot(&json!("x")).is_none());
    }
}
