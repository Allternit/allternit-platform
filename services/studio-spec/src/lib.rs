//! Freeweights Studio contract (Phase 3, step 1).
//!
//! Job spec v1, job states, job and event records, engine descriptions, and a
//! fake engine for other lanes to build against. The HTTP shapes live in
//! `openapi/studio-api.yaml` (host) and `openapi/engine-api.yaml` (engine).

pub mod engines;
#[cfg(feature = "fake-engine")]
pub mod fake_engine;
pub mod job;
pub mod spec;

pub use engines::{catalog, EngineInfo, InstallState};
pub use job::{Job, JobEvent, JobOutput, JobState, TransitionError};
pub use spec::{JobSpec, ValidationError, SPEC_VERSION};

/// Error body for every Studio and engine API: `{ "error": code, "message": text }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApiError {
    pub error: String,
    pub message: String,
}
