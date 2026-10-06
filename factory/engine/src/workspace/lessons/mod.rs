//! Lesson triage: vault memory candidates → (optional System One scoring) →
//! Brain drafts for human approval. Beacon pattern; Raven audit S13 slice.

pub mod candidate;
pub mod sink;
pub mod triage;

pub use candidate::{extract_candidate, is_failure_status, CandidateStatus, MemoryCandidate};
pub use sink::{
    run_memory_sink_contract, InMemorySink, MemorySink, SubmitOutcome, VaultCandidateSink,
};
pub use triage::{triage_dag, TriageConfig, TriageResult, Verdict};
