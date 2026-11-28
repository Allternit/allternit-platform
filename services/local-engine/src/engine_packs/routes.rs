//! Engine pack routes.
//!
//! - `GET  /engines/packs`            installed packs, install jobs, feed info
//! - `POST /engines/packs/ensure`     install the right pack(s) for this machine
//!                                    if missing (body `{"engines":["llamacpp","mlx","colibri"],
//!                                    "update":false}`; default: llama.cpp, plus MLX
//!                                    on Apple silicon, plus colibri where a pack is
//!                                    built (it is small; models are separate); `update` also re-checks the
//!                                    feed for newer versions of installed packs)
//! - `POST /engines/packs/install`    install one pack by id (`{"pack_id": ...}`)
//! - `DELETE /engines/packs/:pack_id` remove a pack (409 while a runtime uses it)
//!
//! Installs run in the background; progress shows in `GET /engines/packs`
//! and `GET /status` (`engine_packs`).

use super::pick::{self, PackChoice, PickInput};
use super::{InstallJob, InstalledPack, PackError};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/engines/packs", get(list))
        .route("/engines/packs/ensure", post(ensure))
        .route("/engines/packs/install", post(install))
        .route("/engines/packs/:pack_id", delete(remove))
        .with_state(state)
}

#[derive(Debug, Serialize)]
pub struct PacksResponse {
    pub platform: String,
    /// True when a debug build reads a local dev feed.
    pub dev_feed: bool,
    pub installed: Vec<InstalledPack>,
    pub jobs: Vec<InstallJob>,
    /// Best-first choices for this machine (only on `ensure`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<PackChoice>>,
    /// Packs `ensure` started installing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub started: Vec<String>,
}

async fn snapshot(state: &AppState) -> PacksResponse {
    let packs = state.packs.clone();
    let installed = tokio::task::spawn_blocking(move || packs.list_installed()).await.unwrap_or_default();
    PacksResponse {
        platform: state.packs.platform().to_string(),
        dev_feed: state.packs.dev_source(),
        installed,
        jobs: state.packs.jobs().await,
        choices: None,
        started: vec![],
    }
}

async fn list(State(state): State<Arc<AppState>>) -> Json<PacksResponse> {
    Json(snapshot(&state).await)
}

#[derive(Debug, Default, Deserialize)]
pub struct EnsureRequest {
    #[serde(default)]
    pub engines: Option<Vec<String>>,
    #[serde(default)]
    pub update: bool,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, e: impl ToString) -> ApiError {
    (status, Json(json!({ "error": e.to_string() })))
}

fn status_for(e: &PackError) -> StatusCode {
    match e {
        PackError::Busy(_) => StatusCode::CONFLICT,
        PackError::Invalid(_) => StatusCode::BAD_REQUEST,
        PackError::NotPublished(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn pick_input() -> PickInput {
    tokio::task::spawn_blocking(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let gpus = crate::hardware::gpu_probe::probe_gpus(sys.total_memory(), sys.available_memory());
        let apple = gpus.iter().any(|g| g.source == crate::hardware::gpu_probe::GpuSource::AppleUnified);
        PickInput::detect(&gpus, apple)
    })
    .await
    .unwrap_or_else(|_| PickInput {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        gpus: vec![],
        apple_silicon: false,
        nvidia_driver: false,
        rocm_runtime: false,
        vulkan_loader: false,
    })
}

async fn ensure(
    State(state): State<Arc<AppState>>,
    body: Option<Json<EnsureRequest>>,
) -> Result<(StatusCode, Json<PacksResponse>), ApiError> {
    let input = pick_input().await;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let update = body.update;
    let wanted = body.engines.unwrap_or_else(|| vec!["llamacpp".into(), "mlx".into(), "colibri".into()]);
    let packs = state.packs.clone();
    let installed = tokio::task::spawn_blocking(move || packs.list_installed()).await.unwrap_or_default();
    let jobs = state.packs.jobs().await;

    let mut all_choices = Vec::new();
    let mut started = Vec::new();
    for engine in wanted {
        let choices: Vec<PackChoice> = match engine.as_str() {
            "llamacpp" | "llama.cpp" => pick::llamacpp_choices(&input),
            "mlx" => pick::mlx_choice(&input).into_iter().collect(),
            "colibri" => pick::colibri_choices(&input),
            other => return Err(err(StatusCode::BAD_REQUEST, format!("unknown engine {other}"))),
        };
        if choices.is_empty() {
            continue;
        }
        all_choices.extend(choices.iter().cloned());
        let busy = jobs.iter().any(|j| {
            choices.iter().any(|c| c.pack_id == j.pack_id)
                && matches!(j.state.as_str(), "queued" | "downloading" | "verifying")
        });
        if busy {
            continue;
        }
        // Installed already: done, unless asked to look for a newer version
        // (an install of the current version is a no-op after a verify).
        let have = installed.iter().find(|p| choices.iter().any(|c| c.pack_id == p.pack_id));
        let ids: Vec<String> = match have {
            Some(p) if update => vec![p.pack_id.clone()],
            Some(_) => continue,
            None => choices.iter().map(|c| c.pack_id.clone()).collect(),
        };
        match state.packs.start_install(ids).await {
            Ok(id) => started.push(id),
            Err(PackError::Busy(_)) => {}
            Err(e) => return Err(err(status_for(&e), e)),
        }
    }
    let mut res = snapshot(&state).await;
    res.choices = Some(all_choices);
    let code = if started.is_empty() { StatusCode::OK } else { StatusCode::ACCEPTED };
    res.started = started;
    Ok((code, Json(res)))
}

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub pack_id: String,
}

async fn install(
    State(state): State<Arc<AppState>>,
    Json(body): Json<InstallRequest>,
) -> Result<(StatusCode, Json<PacksResponse>), ApiError> {
    let id = state
        .packs
        .start_install(vec![body.pack_id])
        .await
        .map_err(|e| err(status_for(&e), e))?;
    let mut res = snapshot(&state).await;
    res.started = vec![id];
    Ok((StatusCode::ACCEPTED, Json(res)))
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(pack_id): Path<String>,
) -> Result<Json<PacksResponse>, ApiError> {
    if state.manager.packs_in_use().await.contains(&pack_id) {
        return Err(err(StatusCode::CONFLICT, format!("{pack_id} is running a model; stop it first")));
    }
    match state.packs.remove(&pack_id).await {
        Ok(true) => Ok(Json(snapshot(&state).await)),
        Ok(false) => Err(err(StatusCode::NOT_FOUND, format!("{pack_id} is not installed"))),
        Err(e) => Err(err(status_for(&e), e)),
    }
}
