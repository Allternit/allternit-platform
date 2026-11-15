//! # Allternit Local Engine
//!
//! Downloads models (verified against the Freeweights manifest or the
//! publisher's hashes, via `allternit-model-source`) and manages local
//! inference runtimes.
//!
//! ## Modules
//!
//! - `cache`: metadata storage for downloaded models.
//! - `download`: file selection, verified background downloads and
//!   `/models/verify` re-hashing.
//! - `runtime`: process manager that launches backends such as llama.cpp and
//!   polls their health endpoints.
//! - `routes`: axum HTTP handlers for `/models/download` and `/status`.

pub mod assess;
pub mod cache;
pub mod catalog;
pub mod download;
pub mod hardware;
pub mod recommend;
pub mod routes;
pub mod runtime;
pub mod sampler;

use std::path::PathBuf;

// Re-export the most commonly used types.
pub use assess::{AssessRequest, AssessResponse, Assessor};
pub use cache::{CachedModel, ModelSource, ModelStatus, ModelStore};
pub use catalog::{CatalogEntry, CatalogService, CatalogSource};
pub use download::{build_cached_model, spawn_download_task, HuggingFaceError, VerifyReport};
pub use hardware::HardwareProfile;
pub use recommend::{RecommendRequest, RecommendResponse, Recommender};
pub use runtime::{ProcessManager, RuntimeInfo, RuntimeManagerError, RuntimeRecipe, RuntimeStatus};
pub use sampler::{SystemSample, SystemSampler};

/// Shared application state used by all HTTP routes.
#[derive(Clone)]
pub struct AppState {
    pub store: ModelStore,
    pub manager: ProcessManager,
    pub models_dir: PathBuf,
    pub data_dir: PathBuf,
    pub hardware_profile: HardwareProfile,
    pub catalog: CatalogService,
    pub assessor: Assessor,
    pub recommender: Recommender,
    pub sampler: SystemSampler,
}
