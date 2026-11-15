//! Model download and cache routes.

use crate::cache::model::{CachedModel, FileVerification, ModelSource};
use crate::cache::store::ModelStore;
use crate::download::huggingface::validate_repo_id;
use crate::download::task::{build_cached_model, spawn_download_task, verify_local, VerifyReport};
use allternit_model_source::{SourceKind, Verification};
use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// Request body for `POST /models/download`.
#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    pub repo_id: String,
    #[serde(default = "default_revision")]
    pub revision: String,
    pub quantization: Option<String>,
    /// Let files with no hash anywhere (no manifest entry, no publisher
    /// hash) download with state `unverified`. Off by default: such a file
    /// fails the download.
    #[serde(default)]
    pub allow_unverified: bool,
}

fn default_revision() -> String {
    "main".to_string()
}

/// Request body for `POST /models/import`.
#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub path: String,
    pub name: Option<String>,
    #[serde(default)]
    pub source: ImportSource,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSource {
    #[default]
    LocalPath,
    UnslothOutput,
    StudioOutput,
}

impl From<ImportSource> for ModelSource {
    fn from(value: ImportSource) -> Self {
        match value {
            ImportSource::LocalPath => ModelSource::LocalPath,
            ImportSource::UnslothOutput => ModelSource::UnslothOutput,
            ImportSource::StudioOutput => ModelSource::StudioOutput,
        }
    }
}

/// Request body for `POST /models/studio-output` (sent by allternit-api's
/// `/api/v1/studio/jobs/{id}/outputs/register`).
#[derive(Debug, Deserialize)]
pub struct StudioOutputRequest {
    /// Adapter/model folder, or a single file (a GGUF).
    pub path: String,
    pub name: String,
    pub job_id: String,
    /// `adapter`, `merged`, `gguf`, `mlx`.
    pub kind: String,
}

/// Request body for `POST /models/verify`.
#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    /// `repo` or `repo_id` (same field).
    #[serde(alias = "repo_id")]
    pub repo: String,
    /// Accepted for symmetry with `/models/download`; every local file of the
    /// repo is checked regardless.
    #[serde(default)]
    #[allow(dead_code)]
    pub quantization: Option<String>,
    /// Defaults to the tracked download's revision, else the catalog's pin / `main`.
    pub revision: Option<String>,
}

/// Response returned after queueing a download (and for an already-tracked
/// one, which then carries its progress and verification).
#[derive(Debug, Serialize)]
pub struct DownloadResponse {
    pub id: String,
    pub repo_id: String,
    pub revision: String,
    pub status: String,
    pub message: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// Contract `verification`; `null` until the download finishes.
    pub verification: Option<Verification>,
    pub source_used: Option<SourceKind>,
    pub error: Option<String>,
}

impl DownloadResponse {
    fn from_model(model: &CachedModel, message: &str) -> Self {
        Self {
            id: model.id.clone(),
            repo_id: model.repo_id.clone(),
            revision: model.revision.clone(),
            status: model.status.to_string(),
            message: message.into(),
            downloaded_bytes: model.downloaded_bytes,
            total_bytes: model.total_bytes,
            verification: model.verification.clone(),
            source_used: model.verification.as_ref().map(|v| v.source_used),
            error: model.error_message.clone(),
        }
    }
}

/// API representation of a cached model (matches the frontend contract).
#[derive(Debug, Serialize)]
pub struct CachedModelResponse {
    pub id: String,
    pub name: String,
    pub source: String,
    pub path: String,
    pub status: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub created_at: String,
    /// Contract `verification`; `null` until a download or verify finishes.
    pub verification: Option<Verification>,
    pub source_used: Option<SourceKind>,
    /// Per-file hashes and how each was checked.
    pub files: Vec<FileVerification>,
    pub error: Option<String>,
}

impl From<&CachedModel> for CachedModelResponse {
    fn from(model: &CachedModel) -> Self {
        Self {
            id: model.id.clone(),
            name: model.repo_id.clone(),
            source: model.source.to_string(),
            path: model.path.to_string_lossy().into_owned(),
            status: model.status.to_string(),
            downloaded_bytes: model.downloaded_bytes,
            total_bytes: model.total_bytes,
            created_at: model.created_at.to_rfc3339(),
            verification: model.verification.clone(),
            source_used: model.verification.as_ref().map(|v| v.source_used),
            files: model.files.clone(),
            error: model.error_message.clone(),
        }
    }
}

/// Wrapper for the list response expected by the frontend.
#[derive(Debug, Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<CachedModelResponse>,
}

/// Error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Create the model cache router.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/models", get(list_models))
        .route("/models/download", post(download_model))
        .route("/models/import", post(import_model))
        .route("/models/verify", post(verify_model))
        .route("/models/studio-output", post(register_studio_output))
        .with_state(state)
}

/// Re-hash a downloaded repo's local files against the signed manifest (or
/// the publisher's hashes) and return the contract `verification`.
async fn verify_model(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<VerifyRequest>,
) -> Result<Json<VerifyReport>, (StatusCode, Json<ErrorResponse>)> {
    let repo = payload.repo.trim().to_string();
    if let Err(err) = validate_repo_id(&repo) {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: err.to_string() })));
    }
    match verify_local(&state.store, &state.models_dir, &repo, payload.revision.as_deref()).await {
        Ok(report) => Ok(Json(report)),
        Err(error) if error.ends_with("is not downloaded") => {
            Err((StatusCode::NOT_FOUND, Json(ErrorResponse { error })))
        }
        Err(error) => Err((StatusCode::BAD_GATEWAY, Json(ErrorResponse { error }))),
    }
}

async fn list_models(State(state): State<Arc<AppState>>) -> Json<ListModelsResponse> {
    let models = state.store.list().await;
    Json(ListModelsResponse {
        models: models.iter().map(CachedModelResponse::from).collect(),
    })
}

async fn import_model(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ImportRequest>,
) -> Result<Json<CachedModelResponse>, (StatusCode, Json<ErrorResponse>)> {
    let path = std::path::PathBuf::from(&payload.path);

    if path.as_os_str().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "path must not be empty".into(),
            }),
        ));
    }

    let name = payload
        .name
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("imported-model")
                .to_string()
        });

    // Verify the path exists and is a directory.
    match tokio::fs::metadata(&path).await {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "import path must be a directory".into(),
                }),
            ));
        }
        Err(err) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("cannot access import path: {}", err),
                }),
            ));
        }
    }

    let id = format!("local--{}--{}", name, Utc::now().timestamp_millis());
    let source: ModelSource = payload.source.into();
    let model = CachedModel::new_local(id.clone(), name, path, source);

    if state.store.get(&id).await.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "model import already tracked".into(),
            }),
        ));
    }

    state.store.insert(model).await;
    info!(%id, "model imported");

    let imported = state.store.get(&id).await.unwrap();
    Ok(Json(CachedModelResponse::from(&imported)))
}

/// List a finished Studio job's output in the local cache (source
/// `studio_output`, status `ready`). Registering the same job output again
/// returns the existing record.
pub async fn studio_output(
    store: &ModelStore,
    req: StudioOutputRequest,
) -> Result<CachedModel, (StatusCode, String)> {
    let safe = |s: &str| -> bool {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    };
    if !safe(&req.job_id) || !safe(&req.kind) {
        return Err((StatusCode::BAD_REQUEST, "job_id and kind must be plain ids".into()));
    }
    let path = std::path::PathBuf::from(&req.path);
    if !path.is_absolute() {
        return Err((StatusCode::BAD_REQUEST, "path must be absolute".into()));
    }
    if let Err(err) = tokio::fs::metadata(&path).await {
        return Err((StatusCode::BAD_REQUEST, format!("cannot access output path: {err}")));
    }
    let id = format!("studio--{}--{}", req.job_id, req.kind);
    let model = match store.get(&id).await {
        Some(existing) => existing,
        None => {
            let name = if req.name.trim().is_empty() { id.clone() } else { req.name.trim().to_string() };
            let mut model = CachedModel::new_local(id.clone(), name, path, ModelSource::StudioOutput);
            model.revision = req.job_id;
            model.set_ready();
            store.insert(model.clone()).await;
            info!(%id, "studio output registered");
            model
        }
    };
    // Kept across restarts: the output lives in its job folder, not models/.
    // Saved on a repeat too, so a retry after a failed save persists it.
    store.save_studio_outputs().await.map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not save the studio output list: {err}"),
        )
    })?;
    Ok(model)
}

async fn register_studio_output(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<StudioOutputRequest>,
) -> Result<Json<CachedModelResponse>, (StatusCode, Json<ErrorResponse>)> {
    studio_output(&state.store, payload)
        .await
        .map(|m| Json(CachedModelResponse::from(&m)))
        .map_err(|(code, error)| (code, Json(ErrorResponse { error })))
}

async fn download_model(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DownloadRequest>,
) -> Result<Json<DownloadResponse>, (StatusCode, Json<ErrorResponse>)> {
    let repo_id = payload.repo_id.trim().to_string();

    if let Err(err) = validate_repo_id(&repo_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        ));
    }

    let model_id =
        ModelStore::model_id(&repo_id, &payload.revision, payload.quantization.as_deref());

    // If the model is already known, return its current state without re-spawning.
    if let Some(existing) = state.store.get(&model_id).await {
        // A failed download is retried; anything else reports its state.
        if existing.status != crate::cache::model::ModelStatus::Failed {
            info!(%model_id, "download already exists");
            return Ok(Json(DownloadResponse::from_model(
                &existing,
                "model download already tracked",
            )));
        }
    }

    let model = build_cached_model(
        &repo_id,
        &payload.revision,
        payload.quantization,
        &state.models_dir,
    );
    let quantization = model.quantization.clone();
    let response = DownloadResponse::from_model(&model, "download queued");

    state.store.insert(model).await;

    spawn_download_task(
        state.store.clone(),
        model_id.clone(),
        repo_id,
        payload.revision,
        quantization,
        payload.allow_unverified,
        state.models_dir.clone(),
    );

    info!(%model_id, "download task spawned");
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn studio_output_is_listed_ready_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let adapter = tmp.path().join("adapter");
        std::fs::create_dir_all(&adapter).unwrap();
        let store = ModelStore::new(tmp.path().join("models"));
        let req = |path: &std::path::Path| StudioOutputRequest {
            path: path.to_string_lossy().into_owned(),
            name: "qwen-lora".into(),
            job_id: "job_1".into(),
            kind: "adapter".into(),
        };

        let model = studio_output(&store, req(&adapter)).await.unwrap();
        assert_eq!(model.id, "studio--job_1--adapter");
        let listed = CachedModelResponse::from(&store.get(&model.id).await.unwrap());
        assert_eq!(listed.source, "studio_output");
        assert_eq!(listed.status, "ready");
        assert_eq!(listed.name, "qwen-lora");

        let again = studio_output(&store, req(&adapter)).await.unwrap();
        assert_eq!(again.created_at, model.created_at);
        assert_eq!(store.list().await.len(), 1);

        let missing = studio_output(&store, req(&tmp.path().join("nope"))).await;
        assert_eq!(missing.unwrap_err().0, StatusCode::BAD_REQUEST);
        let mut traversal = req(&adapter);
        traversal.job_id = "../x".into();
        assert!(studio_output(&store, traversal).await.is_err());
    }

    #[tokio::test]
    async fn studio_outputs_survive_a_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        let kept = tmp.path().join("job_1").join("adapter");
        let removed = tmp.path().join("job_2").join("model.gguf");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::create_dir_all(removed.parent().unwrap()).unwrap();
        std::fs::write(&removed, b"gguf").unwrap();

        let store = ModelStore::new(&models_dir);
        for (path, job_id, kind) in [(&kept, "job_1", "adapter"), (&removed, "job_2", "gguf")] {
            let req = StudioOutputRequest {
                path: path.to_string_lossy().into_owned(),
                name: format!("{job_id}-{kind}"),
                job_id: job_id.into(),
                kind: kind.into(),
            };
            studio_output(&store, req).await.unwrap();
        }
        drop(store);

        // Restart: a fresh store on the same data dir lists both again.
        let restarted = ModelStore::new(&models_dir);
        assert_eq!(restarted.load_studio_outputs().await, 2);
        let model = restarted.get("studio--job_1--adapter").await.unwrap();
        let listed = CachedModelResponse::from(&model);
        assert_eq!((listed.source.as_str(), listed.status.as_str()), ("studio_output", "ready"));
        assert_eq!(listed.name, "job_1-adapter");

        // An output whose files were deleted is not listed after the next restart.
        std::fs::remove_file(&removed).unwrap();
        let again = ModelStore::new(&models_dir);
        assert_eq!(again.load_studio_outputs().await, 1);
        assert!(again.get("studio--job_2--gguf").await.is_none());
        let saved: Vec<CachedModel> =
            serde_json::from_slice(&std::fs::read(again.studio_outputs_file()).unwrap()).unwrap();
        assert_eq!(saved.len(), 1);
    }
}
