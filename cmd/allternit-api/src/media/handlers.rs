//! HTTP handlers for the media plane, plus transport-injected core logic so
//! the whole submit/poll/download pipeline is unit-testable without a network.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::auth::get_user;
use crate::db::DbHandle;
use crate::media::catalog;
use crate::media::clients::{
    self, ImageEntry, MediaTransport, ReqwestTransport, VideoBackend, VideoJobHandle, VideoJobSubmit,
    VideoPoll,
};
use crate::media::{
    self, get_artifact, get_job, insert_artifact, insert_job, resolve_provider_key,
    update_job_status, MediaJobRow,
};
use crate::AppState;

/// `(status, json body)` error used by the core functions.
type CoreError = (StatusCode, Value);

fn no_key_error(credential_provider_id: &str) -> CoreError {
    (
        StatusCode::BAD_REQUEST,
        json!({
            "error": "no_provider_key",
            "message": format!(
                "Add a {credential_provider_id} API key in Settings → Media providers, \
                 or ask the operator to enable the platform-funded lane."
            ),
        }),
    )
}

fn bad_request(message: &str) -> CoreError {
    (StatusCode::BAD_REQUEST, json!({ "error": "bad_request", "message": message }))
}

// ─── Request bodies ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SubmitVideoRequest {
    pub provider: String,
    pub prompt: String,
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub fast: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateImageRequest {
    pub provider: String,
    pub prompt: String,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub n: Option<u32>,
    /// Optional reference photo as a `data:image/{png,jpeg,webp};base64,...`
    /// URL. When set, gpt-image restyles it (images/edits) instead of
    /// generating from text alone — e.g. turning a photo into a bot sprite.
    #[serde(default)]
    pub reference_image: Option<String>,
}

/// Largest reference photo accepted on image generate (decoded bytes).
pub const MAX_REFERENCE_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// Route body limit for image generate: the base64 reference plus JSON.
pub const MAX_IMAGE_GENERATE_BODY_BYTES: usize = 12 * 1024 * 1024;

/// Validate a reference photo data URL (type, base64, size).
fn validate_reference_image(data_url: &str) -> Result<(), CoreError> {
    let Some((header, payload)) = data_url.split_once(',') else {
        return Err(bad_request("reference_image must be a base64 data URL"));
    };
    let allowed = matches!(
        header,
        "data:image/png;base64" | "data:image/jpeg;base64" | "data:image/webp;base64"
    );
    if !allowed {
        return Err(bad_request("reference_image must be a PNG, JPEG, or WebP base64 data URL"));
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| bad_request("reference_image is not valid base64"))?;
    if bytes.is_empty() {
        return Err(bad_request("reference_image is empty"));
    }
    if bytes.len() > MAX_REFERENCE_IMAGE_BYTES {
        return Err(bad_request("reference_image is larger than 8 MB"));
    }
    Ok(())
}

// ─── Core logic (transport-injected) ─────────────────────────────────────────

fn build_submit(req: &SubmitVideoRequest) -> Result<(VideoBackend, VideoJobSubmit, f64), CoreError> {
    let backend = VideoBackend::from_provider_id(&req.provider)
        .ok_or_else(|| bad_request(&format!("unknown provider `{}`", req.provider)))?;
    if req.prompt.is_empty() || req.prompt.chars().count() > 7000 {
        return Err(bad_request("prompt must be 1-7000 characters"));
    }
    let duration = req.duration.unwrap_or(6);
    if !(4..=15).contains(&duration) {
        return Err(bad_request("duration must be between 4 and 15 seconds"));
    }
    if let Some(image_url) = &req.image_url {
        if !image_url.starts_with("https://") {
            return Err(bad_request("image_url must be an https URL"));
        }
    }
    let resolution = req
        .resolution
        .clone()
        .unwrap_or_else(|| match backend {
            VideoBackend::MinimaxH3 => "768P".to_string(),
            VideoBackend::FalSeedance => "720p".to_string(),
        });
    let submit = VideoJobSubmit {
        prompt: req.prompt.clone(),
        duration_secs: duration,
        resolution: resolution.clone(),
        aspect_ratio: req.aspect_ratio.clone().unwrap_or_else(|| "16:9".to_string()),
        image_url: req.image_url.clone(),
        fast: req.fast.unwrap_or(true),
    };
    let cost = catalog::estimate_video_cost(backend.provider_id(), &resolution, submit.fast, duration)
        .ok_or_else(|| bad_request("could not estimate cost for provider"))?;
    Ok((backend, submit, cost))
}

pub async fn submit_video_job_core(
    db: &DbHandle,
    transport: &dyn MediaTransport,
    user_id: &str,
    req: SubmitVideoRequest,
) -> Result<Value, CoreError> {
    let (backend, submit, cost) = build_submit(&req)?;
    let cred_id = media::credential_provider_id(backend.provider_id()).unwrap_or("provider");
    let key = resolve_provider_key(db, user_id, backend.provider_id())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?
        .ok_or_else(|| no_key_error(cred_id))?;

    let handle = clients::submit_video(transport, backend, &key, &submit)
        .await
        .map_err(|message| (StatusCode::BAD_GATEWAY, json!({ "error": "provider_submit_failed", "message": message })))?;

    // Original request params + fal poll URLs ride along in params_json.
    let params_json = json!({
        "duration": submit.duration_secs,
        "resolution": submit.resolution,
        "aspect_ratio": submit.aspect_ratio,
        "image_url": submit.image_url,
        "fast": submit.fast,
        "poll_status_url": handle.poll_status_url,
        "poll_response_url": handle.poll_response_url,
    })
    .to_string();

    let job_id = uuid::Uuid::new_v4().to_string();
    let row = MediaJobRow {
        id: job_id.clone(),
        user_id: user_id.to_string(),
        provider: backend.provider_id().to_string(),
        model: match backend {
            VideoBackend::MinimaxH3 => "MiniMax-H3".to_string(),
            VideoBackend::FalSeedance => "seedance-2.0".to_string(),
        },
        kind: "video".to_string(),
        prompt: req.prompt.clone(),
        params_json: Some(params_json),
        provider_task_id: Some(handle.provider_task_id.clone()),
        status: "queued".to_string(),
        error: None,
        artifact_id: None,
        estimated_cost_usd: Some(cost),
    };
    insert_job(db, &row)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?;

    Ok(json!({
        "job_id": job_id,
        "provider": backend.provider_id(),
        "status": "queued",
        "estimated_cost_usd": cost,
    }))
}

/// Advance an in-flight job one provider poll; download + persist the MP4 on
/// success. Returns the job row (updated when it changed) plus the response
/// payload.
pub async fn poll_job_to_completion(
    db: &DbHandle,
    transport: &dyn MediaTransport,
    row: &MediaJobRow,
) -> Result<(MediaJobRow, Value), CoreError> {
    let backend = VideoBackend::from_provider_id(&row.provider)
        .ok_or_else(|| bad_request("job provider is no longer supported"))?;
    let cred_id = media::credential_provider_id(&row.provider).unwrap_or("provider");
    let key = resolve_provider_key(db, &row.user_id, &row.provider)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?
        .ok_or_else(|| no_key_error(cred_id))?;
    let (status_url, response_url) = row.poll_urls();
    let handle = VideoJobHandle {
        provider_task_id: row.provider_task_id.clone().unwrap_or_default(),
        poll_status_url: status_url,
        poll_response_url: response_url,
    };

    let poll = clients::poll_video(transport, backend, &key, &handle)
        .await
        .map_err(|message| (StatusCode::BAD_GATEWAY, json!({ "error": "provider_poll_failed", "message": message })))?;

    let mut updated = row.clone();
    match poll {
        VideoPoll::Queued => {
            update_job_status(db, &row.id, "queued", None, row.artifact_id.as_deref())
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?;
            updated.status = "queued".to_string();
            Ok((
                updated,
                json!({ "job_id": row.id, "provider": row.provider, "status": "queued" }),
            ))
        }
        VideoPoll::Processing => {
            update_job_status(db, &row.id, "processing", None, row.artifact_id.as_deref())
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?;
            updated.status = "processing".to_string();
            Ok((
                updated,
                json!({ "job_id": row.id, "provider": row.provider, "status": "processing" }),
            ))
        }
        VideoPoll::Failed { message } => {
            update_job_status(db, &row.id, "failed", Some(&message), row.artifact_id.as_deref())
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?;
            updated.status = "failed".to_string();
            updated.error = Some(message.clone());
            Ok((
                updated,
                json!({
                    "job_id": row.id,
                    "provider": row.provider,
                    "status": "failed",
                    "error": message,
                }),
            ))
        }
        VideoPoll::Succeeded { video_url } => {
            let (dl_status, bytes, content_type) = transport
                .get_bytes(&video_url, &[])
                .await
                .map_err(|message| (StatusCode::BAD_GATEWAY, json!({ "error": "artifact_download_failed", "message": message })))?;
            if !(200..300).contains(&dl_status) {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    json!({ "error": "artifact_download_failed", "message": format!("download status {dl_status}") }),
                ));
            }
            let content_type = if content_type.is_empty() {
                "video/mp4".to_string()
            } else {
                content_type
            };
            let artifact_id = insert_artifact(db, &row.user_id, Some(&row.id), &content_type, &bytes)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?;
            update_job_status(db, &row.id, "succeeded", None, Some(&artifact_id))
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?;
            updated.status = "succeeded".to_string();
            updated.artifact_id = Some(artifact_id.clone());
            Ok((
                updated.clone(),
                succeeded_payload(&updated, &artifact_id, &content_type),
            ))
        }
    }
}

fn succeeded_payload(row: &MediaJobRow, artifact_id: &str, content_type: &str) -> Value {
    json!({
        "job_id": row.id,
        "provider": row.provider,
        "status": "succeeded",
        "video": {
            "artifact_url": format!("/api/v1/media/artifacts/{artifact_id}"),
            "content_type": content_type,
        },
        "estimated_cost_usd": row.estimated_cost_usd,
    })
}

/// Load a job and return its payload, polling the provider when the job is
/// still in flight.
pub async fn get_video_job_core(
    db: &DbHandle,
    transport: &dyn MediaTransport,
    user_id: &str,
    job_id: &str,
) -> Result<Value, CoreError> {
    let row = get_job(db, job_id, user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, json!({ "error": "not_found" })))?;

    // Terminal with a persisted artifact — serve the cached payload.
    if row.status == "succeeded" {
        if let Some(artifact_id) = &row.artifact_id {
            let content_type = get_artifact(db, artifact_id, user_id)
                .ok()
                .flatten()
                .map(|a| a.content_type)
                .unwrap_or_else(|| "video/mp4".to_string());
            return Ok(succeeded_payload(&row, artifact_id, &content_type));
        }
    }
    if row.status == "failed" {
        return Ok(json!({
            "job_id": row.id,
            "provider": row.provider,
            "status": "failed",
            "error": row.error,
        }));
    }
    let (_, payload) = poll_job_to_completion(db, transport, &row).await?;
    Ok(payload)
}

/// Resolve a job to its artifact id (polling once when still in flight).
pub async fn job_artifact_core(
    db: &DbHandle,
    transport: &dyn MediaTransport,
    user_id: &str,
    job_id: &str,
) -> Result<(MediaJobRow, String), CoreError> {
    let row = get_job(db, job_id, user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, json!({ "error": "not_found" })))?;
    if row.status != "succeeded" || row.artifact_id.is_none() {
        let (updated, _) = poll_job_to_completion(db, transport, &row).await?;
        if updated.status != "succeeded" {
            return Err((
                StatusCode::CONFLICT,
                json!({ "error": "job_not_succeeded", "status": updated.status }),
            ));
        }
        return Ok((updated.clone(), updated.artifact_id.clone().unwrap_or_default()));
    }
    Ok((row.clone(), row.artifact_id.clone().unwrap()))
}

pub async fn generate_image_core(
    db: &DbHandle,
    transport: &dyn MediaTransport,
    user_id: &str,
    req: GenerateImageRequest,
) -> Result<Value, CoreError> {
    if req.prompt.is_empty() || req.prompt.chars().count() > 7000 {
        return Err(bad_request("prompt must be 1-7000 characters"));
    }
    let n = req.n.unwrap_or(1);
    if !(1..=4).contains(&n) {
        return Err(bad_request("n must be between 1 and 4"));
    }
    if let Some(reference) = &req.reference_image {
        if req.provider != "gpt-image" {
            return Err(bad_request("reference_image is only supported by the gpt-image provider"));
        }
        validate_reference_image(reference)?;
    }
    let cred_id = media::credential_provider_id(&req.provider)
        .ok_or_else(|| bad_request(&format!("unknown provider `{}`", req.provider)))?;
    let key = resolve_provider_key(db, user_id, &req.provider)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?
        .ok_or_else(|| no_key_error(cred_id))?;

    let (entries, estimated_cost) = match req.provider.as_str() {
        "gpt-image" => {
            let quality = req.quality.clone().unwrap_or_else(|| "medium".to_string());
            if !["low", "medium", "high"].contains(&quality.as_str()) {
                return Err(bad_request("quality must be low, medium, or high"));
            }
            let size = req.size.clone().unwrap_or_else(|| "1024x1024".to_string());
            if !["1024x1024", "1024x1536", "1536x1024"].contains(&size.as_str()) {
                return Err(bad_request("size must be 1024x1024, 1024x1536, or 1536x1024"));
            }
            let cost = catalog::estimate_gpt_image_cost(&quality, n)
                .ok_or_else(|| bad_request("unknown quality"))?;
            let entries = match &req.reference_image {
                Some(reference) => {
                    clients::edit_gpt_images(transport, &key, &req.prompt, reference, &size, &quality, n).await
                }
                None => clients::generate_gpt_images(transport, &key, &req.prompt, &size, &quality, n).await,
            }
                .map_err(|message| (StatusCode::BAD_GATEWAY, json!({ "error": "provider_generate_failed", "message": message })))?;
            (entries, cost)
        }
        "flux-fal" => {
            let entries = clients::generate_flux_images(transport, &key, &req.prompt, n, None)
                .await
                .map_err(|message| (StatusCode::BAD_GATEWAY, json!({ "error": "provider_generate_failed", "message": message })))?;
            (entries, catalog::estimate_flux_cost(n))
        }
        other => return Err(bad_request(&format!("provider `{other}` is not an image provider"))),
    };

    let mut images = Vec::new();
    for entry in entries {
        let (bytes, content_type, width, height) = match &entry {
            ImageEntry::B64 { b64_json } => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64_json)
                    .map_err(|e| (StatusCode::BAD_GATEWAY, json!({ "error": "b64_decode_failed", "message": e.to_string() })))?;
                (bytes, "image/png".to_string(), None, None)
            }
            ImageEntry::Url { url, width, height, content_type } => {
                let (status, bytes, ct) = transport
                    .get_bytes(url, &[])
                    .await
                    .map_err(|message| (StatusCode::BAD_GATEWAY, json!({ "error": "image_download_failed", "message": message })))?;
                if !(200..300).contains(&status) {
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        json!({ "error": "image_download_failed", "message": format!("download status {status}") }),
                    ));
                }
                let ct = if ct.is_empty() {
                    content_type.clone().unwrap_or_else(|| "image/png".to_string())
                } else {
                    ct
                };
                (bytes, ct, *width, *height)
            }
        };
        let artifact_id = insert_artifact(db, user_id, None, &content_type, &bytes)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "db", "message": e.to_string() })))?;
        images.push(json!({
            "artifact_url": format!("/api/v1/media/artifacts/{artifact_id}"),
            "width": width,
            "height": height,
        }));
    }

    Ok(json!({ "images": images, "estimated_cost_usd": estimated_cost }))
}

// ─── Axum handlers ───────────────────────────────────────────────────────────

fn transport() -> ReqwestTransport {
    ReqwestTransport::new()
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Unauthorized" }))).into_response()
}

fn core_err(e: CoreError) -> Response {
    (e.0, Json(e.1)).into_response()
}

pub async fn get_media_catalog(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    Json(catalog::catalog_for_user(&state.db, &user.user_id)).into_response()
}

pub async fn submit_video_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<SubmitVideoRequest>,
) -> Response {
    let user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match submit_video_job_core(&state.db, &transport(), &user.user_id, payload).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => core_err(e),
    }
}

pub async fn get_video_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Response {
    let user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match get_video_job_core(&state.db, &transport(), &user.user_id, &job_id).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => core_err(e),
    }
}

pub async fn download_video_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Response {
    let user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match job_artifact_core(&state.db, &transport(), &user.user_id, &job_id).await {
        Ok((_, artifact_id)) => {
            Redirect::temporary(&format!("/api/v1/media/artifacts/{artifact_id}")).into_response()
        }
        Err(e) => core_err(e),
    }
}

pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<GenerateImageRequest>,
) -> Response {
    let user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match generate_image_core(&state.db, &transport(), &user.user_id, payload).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => core_err(e),
    }
}

/// Largest file the app may store (a natively rendered MP4). The route's body
/// limit matches.
pub const MAX_UPLOAD_BYTES: usize = 256 * 1024 * 1024;

/// Store a file the app made itself (the media_generate native lane renders
/// and encodes video in the app). Only media types are accepted, and the
/// artifact is scoped to the uploading user like any other.
pub fn store_uploaded_artifact_core(
    db: &DbHandle,
    user_id: &str,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Value, CoreError> {
    let content_type = content_type
        .map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .unwrap_or_default();
    let allowed = matches!(
        content_type.as_str(),
        "video/mp4" | "video/webm" | "image/png" | "image/jpeg" | "image/webp" | "audio/mp4" | "audio/mpeg" | "audio/wav"
    );
    if !allowed {
        return Err(bad_request("Only video/mp4, video/webm, PNG/JPEG/WebP images and MP4/MP3/WAV audio can be stored"));
    }
    if bytes.is_empty() {
        return Err(bad_request("The file is empty"));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(bad_request("The file is larger than 256 MB"));
    }
    let artifact_id = insert_artifact(db, user_id, None, &content_type, bytes).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "db", "message": e.to_string() }),
        )
    })?;
    Ok(json!({
        "artifact_id": artifact_id,
        "artifact_url": format!("/api/v1/media/artifacts/{artifact_id}"),
        "content_type": content_type,
        "bytes": bytes.len(),
    }))
}

pub async fn upload_media_artifact(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
    match store_uploaded_artifact_core(&state.db, &user.user_id, content_type, &body) {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(e) => core_err(e),
    }
}

pub async fn get_media_artifact(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(artifact_id): Path<String>,
) -> Response {
    let user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    let artifact = match get_artifact(&state.db, &artifact_id, &user.user_id) {
        Ok(Some(artifact)) => artifact,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "db", "message": e.to_string() })),
            )
                .into_response()
        }
    };
    let len = artifact.bytes.len();
    (
        [
            (header::CONTENT_TYPE, artifact.content_type),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        artifact.bytes,
    )
        .into_response()
}
