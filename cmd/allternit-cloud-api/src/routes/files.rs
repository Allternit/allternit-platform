//! User file uploads to the private R2 bucket, with per-plan caps.
//!
//! * `GET    /api/v1/files/usage`: bytes used and the plan's caps.
//! * `POST   /api/v1/files/uploads` `{name, contentType, bytes}`: checks the caps
//!   and the global R2 guard, returns `{fileId, key, putUrl}` (PUT, 15 minutes,
//!   signed for exactly `bytes`). 413 `file-too-large` / `storage-quota-exceeded`,
//!   507 `storage-full`, 503 `storage-unavailable`.
//! * `POST   /api/v1/files/:id/complete` `{name, contentType, bytes}`: HEAD-verifies
//!   the size, then records the file. 409 `size-mismatch` / `upload-missing`.
//! * `GET    /api/v1/files/:id[?expires=SECONDS]`: owner gets a presigned GET
//!   (10 minutes, or up to 7 days when `expires` is given).
//! * `DELETE /api/v1/files/:id`: deletes the object, then the record.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    auth,
    error::ApiError,
    services::{
        r2::R2Client,
        user_files::{self, Failure},
        voice_usage,
    },
    ApiState,
};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/files/usage", get(usage))
        .route("/api/v1/files/uploads", post(begin))
        .route("/api/v1/files/:id/complete", post(complete))
        .route("/api/v1/files/:id", get(download).delete(remove))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileBody {
    name: String,
    #[serde(default)]
    content_type: String,
    bytes: u64,
}

fn reply(f: Failure) -> Result<Response, ApiError> {
    match f {
        Failure::Api(e) => Err(e),
        Failure::Refused(r) => Ok((
            StatusCode::from_u16(r.status).unwrap_or(StatusCode::BAD_REQUEST),
            Json(json!({ "code": r.code, "message": r.message })),
        )
            .into_response()),
    }
}

fn store() -> Result<R2Client, Failure> {
    R2Client::from_env().map_err(|_| {
        Failure::Refused(user_files::Refusal {
            status: 503,
            code: "storage-unavailable",
            message: "File storage is not available right now.".into(),
        })
    })
}

async fn usage(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = auth::resolve_user_id(&state.db, &headers).await?;
    let plan = voice_usage::plan_for_user(&state.db, &user).await?;
    Ok(Json(user_files::usage_json(&state.db, &user, &plan).await?).into_response())
}

async fn begin(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(b): Json<FileBody>) -> Result<Response, ApiError> {
    let user = auth::resolve_user_id(&state.db, &headers).await?;
    let plan = voice_usage::plan_for_user(&state.db, &user).await?;
    let r2 = match store() {
        Ok(r) => r,
        Err(f) => return reply(f),
    };
    match user_files::begin_upload(&state.db, &r2, &user, &plan, &b.name, &b.content_type, b.bytes).await {
        Ok(u) => Ok(Json(json!({ "fileId": u.file_id, "key": u.key, "putUrl": u.put_url, "expiresInSeconds": user_files::PUT_TTL.as_secs() })).into_response()),
        Err(f) => reply(f),
    }
}

async fn complete(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<FileBody>) -> Result<Response, ApiError> {
    let user = auth::resolve_user_id(&state.db, &headers).await?;
    let r2 = match store() {
        Ok(r) => r,
        Err(f) => return reply(f),
    };
    match user_files::complete_upload(&state.db, &r2, &user, id, &b.name, &b.content_type, b.bytes, Utc::now()).await {
        Ok(v) => Ok((StatusCode::CREATED, Json(v)).into_response()),
        Err(f) => reply(f),
    }
}

#[derive(Debug, Deserialize)]
struct DownloadQuery {
    /// Link lifetime in seconds (60 to 7 days). Default 10 minutes.
    expires: Option<u64>,
}

async fn download(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>, Query(q): Query<DownloadQuery>) -> Result<Response, ApiError> {
    let user = auth::resolve_user_id(&state.db, &headers).await?;
    let r2 = match store() {
        Ok(r) => r,
        Err(f) => return reply(f),
    };
    match user_files::download_url(&state.db, &r2, &user, id, q.expires).await {
        Ok(v) => Ok(Json(v).into_response()),
        Err(f) => reply(f),
    }
}

async fn remove(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>) -> Result<Response, ApiError> {
    let user = auth::resolve_user_id(&state.db, &headers).await?;
    let r2 = match store() {
        Ok(r) => r,
        Err(f) => return reply(f),
    };
    match user_files::delete_file(&state.db, &r2, &user, id, Utc::now()).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(f) => reply(f),
    }
}
