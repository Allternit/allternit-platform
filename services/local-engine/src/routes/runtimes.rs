//! Runtime launch/stop routes.

use crate::runtime::{RuntimeInfo, RuntimeRecipe};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// Request body for `POST /runtimes/launch`.
#[derive(Debug, Deserialize)]
pub struct LaunchRequest {
    pub model_id: String,
    pub recipe: RuntimeRecipe,
    pub port: Option<u16>,
}

/// Runtime response DTO that matches the frontend contract.
#[derive(Debug, Serialize)]
pub struct RuntimeInstanceResponse {
    pub id: String,
    pub model_id: String,
    pub recipe: RuntimeRecipe,
    pub pid: Option<u32>,
    pub port: u16,
    pub status: String,
    pub health: Option<RuntimeHealthResponse>,
    /// `pack` (verified engine pack) or `fallback` (binary on PATH).
    pub engine_source: Option<String>,
    pub pack_id: Option<String>,
    pub pack_version: Option<String>,
    pub backend: Option<String>,
    /// Live numbers while running: mem_bytes, tok_per_sec, ctx_used, ctx_max, idle.
    pub running: Option<crate::runtime::RunningInfo>,
    /// `idle` when the engine unloaded it after the idle timeout.
    pub stop_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeHealthResponse {
    pub reachable: bool,
    pub last_check_at: Option<String>,
    pub error: Option<String>,
}

impl From<&RuntimeInfo> for RuntimeInstanceResponse {
    fn from(info: &RuntimeInfo) -> Self {
        Self {
            id: info.id.clone(),
            model_id: info.model_id.clone(),
            recipe: info.recipe_value.clone(),
            pid: info.pid,
            port: info.port,
            status: info.status.to_string(),
            health: info.health.map(|reachable| RuntimeHealthResponse {
                reachable,
                last_check_at: Some(info.updated_at.to_rfc3339()),
                error: info.error_message.clone(),
            }),
            engine_source: info.engine_source.clone(),
            pack_id: info.pack_id.clone(),
            pack_version: info.pack_version.clone(),
            backend: info.backend.clone(),
            running: info.running.clone(),
            stop_reason: info.stop_reason.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ListRuntimesResponse {
    pub runtimes: Vec<RuntimeInstanceResponse>,
}

#[derive(Debug, Serialize)]
pub struct LaunchResponse {
    pub runtime: RuntimeInstanceResponse,
}

/// Error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Create the runtimes router.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/runtimes", get(list_runtimes))
        .route("/runtimes/launch", post(launch_runtime))
        .route("/runtimes/:id/stop", post(stop_runtime))
        .with_state(state)
}

async fn list_runtimes(State(state): State<Arc<AppState>>) -> Json<ListRuntimesResponse> {
    let runtimes = state.manager.list_runtimes().await;
    Json(ListRuntimesResponse {
        runtimes: runtimes.iter().map(RuntimeInstanceResponse::from).collect(),
    })
}

async fn launch_runtime(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LaunchRequest>,
) -> Result<Json<LaunchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let model_id = payload.model_id.trim().to_string();
    if model_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "model_id must not be empty".into(),
            }),
        ));
    }

    match state.manager.launch(model_id.clone(), payload.recipe).await {
        Ok(info) => {
            info!(runtime_id = %info.id, %model_id, "runtime launched");
            Ok(Json(LaunchResponse {
                runtime: RuntimeInstanceResponse::from(&info),
            }))
        }
        Err(err) => Err((
            StatusCode::from_u16(map_runtime_error_status(&err))
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )),
    }
}

async fn stop_runtime(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeInstanceResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.manager.stop_runtime(&id).await {
        Ok(info) => Ok(Json(RuntimeInstanceResponse::from(&info))),
        Err(err) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )),
    }
}

fn map_runtime_error_status(err: &crate::runtime::RuntimeManagerError) -> u16 {
    use crate::runtime::RuntimeManagerError;
    match err {
        RuntimeManagerError::BinaryNotFound(_) | RuntimeManagerError::UnsupportedBackend(_) => 501,
        RuntimeManagerError::PackVerification(_) => 503,
        RuntimeManagerError::NotFound(_) => 404,
        RuntimeManagerError::NoFreePort => 503,
        _ => 500,
    }
}
