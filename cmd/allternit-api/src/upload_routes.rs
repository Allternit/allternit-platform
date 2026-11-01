//! Composer attachment uploads — `POST /api/v1/uploads`.
//!
//! The mobile composer's "+" sheet (Claude iOS parity) stages photos/files
//! before send; this endpoint is where those bytes land. Accepts a JSON
//! envelope `{name, mediaType, dataBase64}` (≤20MB decoded), writes the file
//! under `<data_dir>/uploads/<uploadId>` and returns `{uploadId, url}` where
//! `url` is the fetchable `GET /api/v1/uploads/:id` path. The agent-chat
//! bridge (v1_routes.rs) forwards that URL to the runtime as a
//! `{"type":"file","url":...}` part.

use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

/// Decoded payload cap — large enough for full-res photos, small enough to
/// keep a stray client from filling the data dir.
const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

pub fn upload_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/uploads", post(create_upload))
        .route("/uploads/:id", get(get_upload))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUploadRequest {
    name: String,
    media_type: String,
    data_base64: String,
}

/// Upload directory — same data-dir convention as main.rs / config.rs
/// (ALLTERNIT_DATA_DIR → platform data dir → /var/lib/allternit).
fn uploads_dir() -> PathBuf {
    std::env::var("ALLTERNIT_DATA_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("allternit")))
        .unwrap_or_else(|| PathBuf::from("/var/lib/allternit"))
        .join("uploads")
}

/// Sidecar holding the original media type + filename (the payload file
/// itself is extensionless, keyed by uploadId).
fn meta_path(id: &str) -> PathBuf {
    uploads_dir().join(format!("{}.json", id))
}

/// Read a stored upload's bytes plus its original filename/media type from
/// the sidecar. `pub(crate)` so the tool executor (`tool_routes.rs`) can hand
/// upload bytes to the office engine without re-implementing the storage
/// layout. Returns `None` when the id is invalid or the payload is missing.
pub(crate) fn read_upload(id: &str) -> Option<(Vec<u8>, String, String)> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let bytes = std::fs::read(uploads_dir().join(id)).ok()?;
    let (name, media_type) = std::fs::read_to_string(meta_path(id))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .map(|meta| {
            let name = meta
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("document")
                .to_string();
            let media_type = meta
                .get("mediaType")
                .and_then(|v| v.as_str())
                .unwrap_or("application/octet-stream")
                .to_string();
            (name, media_type)
        })
        .unwrap_or_else(|| ("document".to_string(), "application/octet-stream".to_string()));
    Some((bytes, name, media_type))
}

async fn create_upload(Json(req): Json<CreateUploadRequest>) -> impl IntoResponse {
    let bytes = match STANDARD.decode(req.data_base64.as_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_base64", "message": "dataBase64 is not valid base64."})),
            )
                .into_response();
        }
    };

    if bytes.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "empty_upload", "message": "The upload is empty."})),
        )
            .into_response();
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "too_large", "message": "Uploads are limited to 20MB."})),
        )
            .into_response();
    }

    let id = Uuid::new_v4().simple().to_string();
    let dir = uploads_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "storage_error", "message": e.to_string()})),
        )
            .into_response();
    }

    if let Err(e) = std::fs::write(dir.join(&id), &bytes) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "storage_error", "message": e.to_string()})),
        )
            .into_response();
    }

    let meta = json!({ "name": req.name, "mediaType": req.media_type });
    if let Err(e) = std::fs::write(meta_path(&id), meta.to_string()) {
        // Payload already landed; a missing sidecar only degrades GET's
        // Content-Type, so log-and-continue instead of failing the upload.
        tracing::warn!(error = %e, upload_id = %id, "failed to write upload sidecar");
    }

    Json(json!({
        "uploadId": id,
        "url": format!("/api/v1/uploads/{}", id),
    }))
    .into_response()
}

/// Serves a stored upload so runtime file parts (and clients) can fetch it.
async fn get_upload(headers: HeaderMap, Path(id): Path<String>) -> impl IntoResponse {
    // Upload ids are hex-only (Uuid::simple), but reject anything path-y
    // anyway so this can never escape the uploads dir.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_upload_id"}))).into_response();
    }

    let path = uploads_dir().join(&id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "upload_not_found"}))).into_response();
        }
    };

    let media_type = std::fs::read_to_string(meta_path(&id))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|meta| meta.get("mediaType").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "application/octet-stream".to_string());

    crate::byte_range::ranged_response(&headers, media_type, bytes)
}
