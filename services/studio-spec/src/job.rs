use crate::spec::JobSpec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Preparing,
    Running,
    Exporting,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("job cannot go from {from:?} to {to:?}")]
pub struct TransitionError {
    pub from: JobState,
    pub to: JobState,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// Forward-only: queued -> preparing -> running -> exporting -> succeeded.
    /// Any non-terminal state may go to failed or cancelled. Terminal states
    /// never move.
    pub fn can_transition_to(self, to: JobState) -> bool {
        use JobState::*;
        if self.is_terminal() {
            return false;
        }
        matches!(
            (self, to),
            (Queued, Preparing)
                | (Preparing, Running)
                | (Running, Exporting)
                | (Exporting, Succeeded)
                | (_, Failed)
                | (_, Cancelled)
        )
    }

    pub fn transition(self, to: JobState) -> Result<JobState, TransitionError> {
        if self.can_transition_to(to) {
            Ok(to)
        } else {
            Err(TransitionError { from: self, to })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobEvent {
    /// Starts at 1, strictly increasing per job.
    pub seq: u64,
    /// RFC 3339.
    pub at: chrono::DateTime<chrono::Utc>,
    /// `state`, `log`, `progress`, `output`, `error`
    pub kind: String,
    pub message: String,
    /// 0.0 to 1.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
}

/// A file a job produced. `kind`: `adapter`, `merged`, `gguf`, `mlx`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobOutput {
    pub kind: String,
    /// Absolute path on the computer that ran the job, inside the output dir
    /// the host gave the engine (`ALLTERNIT_STUDIO_OUTPUT_DIR`).
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub state: JobState,
    pub spec: JobSpec,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub error: Option<String>,
    /// Highest event seq so far.
    #[serde(default)]
    pub last_seq: u64,
    #[serde(default)]
    pub outputs: Vec<JobOutput>,
    /// Backend the host picked for this job (`mlx`, `torch-mps`, `torch-cuda`, ...), from `check_job`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}
