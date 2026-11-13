//! Composer attachment uploads — `POST /api/v1/uploads`.
//!
//! The mobile composer's "+" sheet (Claude iOS parity) stages photos/files
//! before send; this endpoint is where those bytes land. Accepts a JSON
//! envelope `{name, mediaType, dataBase64}` (≤20MB decoded), writes the file
//! under `<data_dir>/uploads/<uploadId>` and returns `{uploadId, url}` where
//! `url` is the fetchable `GET /api/v1/uploads/:id` path. The agent-chat
//! bridge (v1_routes.rs) forwards that URL to the runtime as a
//! `{"type":"file","url":...}` part.
//!
//! Serving is locked down for every upload, whoever produced it (a client,
//! run_code, the sandbox): `X-Content-Type-Options: nosniff` and
//! `Content-Security-Policy: sandbox` always, and `Content-Disposition:
//! attachment` for every type except PNG/JPEG/GIF/WebP/PDF (those are
//! `inline`). An uploaded or script-written `.html`/`.svg` can therefore
//! never run as a page on the API origin; clients that want to show one
//! fetch the bytes and render them in a sandboxed frame.

use axum::{
    extract::Path,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path as FsPath, PathBuf};
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
pub(crate) fn uploads_dir() -> PathBuf {
    std::env::var("ALLTERNIT_DATA_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("allternit")))
        .unwrap_or_else(|| PathBuf::from("/var/lib/allternit"))
        .join("uploads")
}

/// A fresh upload id (hex `Uuid::simple`) — every upload, client-sent or
/// server-produced, gets one of these so ids never encode a path or content.
pub(crate) fn new_upload_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// The fetchable path for upload `id`.
pub(crate) fn upload_url(id: &str) -> String {
    format!("/api/v1/uploads/{}", id)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Sidecar holding the original media type + filename (the payload file
/// itself is extensionless, keyed by uploadId).
fn meta_path_in(dir: &FsPath, id: &str) -> PathBuf {
    dir.join(format!("{}.json", id))
}

/// Read a stored upload's bytes plus its original filename/media type from
/// the sidecar. `pub(crate)` so the tool executor (`tool_routes.rs`) can hand
/// upload bytes to the office engine without re-implementing the storage
/// layout. Returns `None` when the id is invalid or the payload is missing.
pub(crate) fn read_upload(id: &str) -> Option<(Vec<u8>, String, String)> {
    read_upload_in(&uploads_dir(), id)
}

fn read_upload_in(dir: &FsPath, id: &str) -> Option<(Vec<u8>, String, String)> {
    if !valid_id(id) {
        return None;
    }
    let bytes = std::fs::read(dir.join(id)).ok()?;
    let (name, media_type) = std::fs::read_to_string(meta_path_in(dir, id))
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

/// Write bytes as upload `id` (payload + sidecar). Shared by `POST /uploads`
/// and server-side ingestion (run_code output files, sandbox `files[]`), so
/// every produced file is fetchable at `GET /api/v1/uploads/:id`. The id must
/// be alphanumeric; a missing sidecar only degrades the served media type, so
/// a sidecar write failure is logged, not returned.
pub(crate) fn store_upload(id: &str, name: &str, media_type: &str, bytes: &[u8]) -> std::io::Result<()> {
    store_upload_in(&uploads_dir(), id, name, media_type, bytes)
}

/// `store_upload` into an explicit uploads dir (run_code ingest passes the
/// dir it was resolved with, so ingest and its manifest agree).
pub(crate) fn store_upload_in(
    dir: &FsPath,
    id: &str,
    name: &str,
    media_type: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    if !valid_id(id) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid upload id"));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "uploads are limited to 20MB"));
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(id), bytes)?;
    let meta = json!({ "name": name, "mediaType": media_type });
    if let Err(e) = std::fs::write(meta_path_in(dir, id), meta.to_string()) {
        tracing::warn!(error = %e, upload_id = %id, "failed to write upload sidecar");
    }
    Ok(())
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

    let id = new_upload_id();
    if let Err(e) = store_upload(&id, &req.name, &req.media_type, &bytes) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "storage_error", "message": e.to_string()})),
        )
            .into_response();
    }

    Json(json!({
        "uploadId": id,
        "url": upload_url(&id),
    }))
    .into_response()
}

/// Best-effort removal of an upload (payload + sidecar) — used when a
/// run_code ingest loses the race to write its manifest.
pub(crate) fn remove_upload_in(dir: &FsPath, id: &str) {
    if valid_id(id) {
        let _ = std::fs::remove_file(dir.join(id));
        let _ = std::fs::remove_file(meta_path_in(dir, id));
    }
}

/// Types a browser may show in place: raster images and PDF. Everything else
/// (HTML, SVG, XML, JS, text, office files, …) downloads.
fn inline_safe(media_type: &str) -> bool {
    let essence = media_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "application/pdf"
    )
}

/// Protective headers for served upload bytes: `nosniff` + `CSP: sandbox`
/// always, `Content-Disposition` inline for raster images/PDF and
/// attachment for everything else (RFC 6266 `filename*` with the stored name).
pub(crate) fn protect_upload_response(response: &mut Response, media_type: &str, name: &str) {
    let headers = response.headers_mut();
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
    let disposition = if inline_safe(media_type) { "inline" } else { "attachment" };
    let name = if name.trim().is_empty() { "download" } else { name };
    // urlencoding leaves only ALPHA / DIGIT / "-._~" unescaped — all RFC 5987 attr-chars.
    let value = format!("{disposition}; filename*=UTF-8''{}", urlencoding::encode(name));
    let value = HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    headers.insert(header::CONTENT_DISPOSITION, value);
}

/// Serves a stored upload so runtime file parts (and clients) can fetch it.
async fn get_upload(headers: HeaderMap, Path(id): Path<String>) -> Response {
    serve_upload_in(&uploads_dir(), &headers, &id)
}

fn serve_upload_in(dir: &FsPath, headers: &HeaderMap, id: &str) -> Response {
    // Upload ids are hex-only (Uuid::simple), but reject anything path-y
    // anyway so this can never escape the uploads dir.
    if !valid_id(id) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_upload_id"}))).into_response();
    }
    let Some((bytes, name, media_type)) = read_upload_in(dir, id) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "upload_not_found"}))).into_response();
    };
    let mut response = crate::byte_range::ranged_response(headers, media_type.clone(), bytes);
    protect_upload_response(&mut response, &media_type, &name);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn router(dir: PathBuf) -> Router {
        Router::new().route(
            "/api/v1/uploads/:id",
            get(move |headers: HeaderMap, Path(id): Path<String>| {
                let dir = dir.clone();
                async move { serve_upload_in(&dir, &headers, &id) }
            }),
        )
    }

    async fn fetch(dir: &FsPath, id: &str, range: Option<&str>) -> Response {
        let mut req = Request::get(format!("/api/v1/uploads/{id}"));
        if let Some(r) = range {
            req = req.header(header::RANGE, r);
        }
        router(dir.to_path_buf()).oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
    }

    fn header_of(r: &Response, name: header::HeaderName) -> String {
        r.headers().get(name).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
    }

    #[tokio::test]
    async fn every_upload_gets_nosniff_csp_sandbox_and_a_disposition_by_type() {
        let tmp = tempfile::tempdir().unwrap();
        let cases = [
            ("page.html", "text/html", "attachment"),
            ("logo.svg", "image/svg+xml", "attachment"),
            ("notes.txt", "text/plain", "attachment"),
            ("data.json", "application/json", "attachment"),
            ("x.bin", "application/octet-stream", "attachment"),
            ("chart.png", "image/png", "inline"),
            ("photo.jpg", "image/jpeg", "inline"),
            ("anim.gif", "image/gif", "inline"),
            ("pic.webp", "image/webp", "inline"),
            ("report.pdf", "application/pdf", "inline"),
            ("PIC.PNG", "IMAGE/PNG; charset=binary", "inline"),
        ];
        for (name, media_type, disposition) in cases {
            let id = new_upload_id();
            store_upload_in(tmp.path(), &id, name, media_type, b"<html><script>1</script>").unwrap();
            let r = fetch(tmp.path(), &id, None).await;
            assert_eq!(r.status(), StatusCode::OK, "{name}");
            assert_eq!(header_of(&r, header::CONTENT_TYPE), media_type, "{name}");
            assert_eq!(header_of(&r, header::X_CONTENT_TYPE_OPTIONS), "nosniff", "{name}");
            assert_eq!(header_of(&r, header::CONTENT_SECURITY_POLICY), "sandbox", "{name}");
            let cd = header_of(&r, header::CONTENT_DISPOSITION);
            assert!(cd.starts_with(&format!("{disposition}; filename*=UTF-8''")), "{name}: {cd}");
        }
    }

    #[tokio::test]
    async fn disposition_encodes_the_name_and_ranges_keep_the_headers() {
        let tmp = tempfile::tempdir().unwrap();
        let id = new_upload_id();
        store_upload_in(tmp.path(), &id, "Q3 \"report\";\r\nx=1 é.html", "text/html", b"0123456789").unwrap();
        let r = fetch(tmp.path(), &id, Some("bytes=0-3")).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(header_of(&r, header::CONTENT_SECURITY_POLICY), "sandbox");
        assert_eq!(
            header_of(&r, header::CONTENT_DISPOSITION),
            "attachment; filename*=UTF-8''Q3%20%22report%22%3B%0D%0Ax%3D1%20%C3%A9.html"
        );
    }

    #[tokio::test]
    async fn missing_sidecar_serves_octet_stream_as_attachment() {
        let tmp = tempfile::tempdir().unwrap();
        let id = new_upload_id();
        std::fs::write(tmp.path().join(&id), b"<svg/>").unwrap();
        let r = fetch(tmp.path(), &id, None).await;
        assert_eq!(header_of(&r, header::CONTENT_TYPE), "application/octet-stream");
        assert_eq!(header_of(&r, header::CONTENT_DISPOSITION), "attachment; filename*=UTF-8''document");
        assert_eq!(fetch(tmp.path(), "nope", None).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(fetch(tmp.path(), "..%2Fx", None).await.status(), StatusCode::BAD_REQUEST);
    }
}
