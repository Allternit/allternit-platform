//! Freeweights Studio contract (Phase 3 step 1; Phase 4 step 1: every training backend).
//!
//! Job spec v1, job states, job and event records, engine descriptions, and a
//! fake engine for other lanes to build against. The HTTP shapes live in
//! `openapi/studio-api.yaml` (host) and `openapi/engine-api.yaml` (engine).

pub mod engines;
#[cfg(feature = "fake-engine")]
pub mod fake_engine;
pub mod fit;
pub mod hardware;
pub mod job;
pub mod spec;

pub use engines::{catalog, EngineInfo, EngineSource, InstallState};
pub use job::{Job, JobEvent, JobOutput, JobState, TransitionError};
pub use fit::{check_job, fit, Fit, Selected};
pub use hardware::{classify, GpuVendor, Hardware, HostHardware, INTEL_MAC_NO_TORCH};
pub use spec::{BackendChoice, JobSpec, ValidationError, SPEC_VERSION};

/// Error body for every Studio and engine API: `{ "error": code, "message": text }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApiError {
    pub error: String,
    pub message: String,
}
