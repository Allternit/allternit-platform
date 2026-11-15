//! Cached model state.

use allternit_model_source::Verification;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Current lifecycle status of a cached model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// Download has been requested but not yet started.
    Pending,
    /// Files are actively being downloaded.
    Downloading,
    /// All files are present and ready for use.
    Ready,
    /// Download or verification failed.
    Failed,
}

impl std::fmt::Display for ModelStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelStatus::Pending => write!(f, "pending"),
            ModelStatus::Downloading => write!(f, "downloading"),
            ModelStatus::Ready => write!(f, "ready"),
            ModelStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Where the cached model originally came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// Downloaded from the Hugging Face Hub.
    HuggingFace,
    /// Imported from a local filesystem path.
    LocalPath,
    /// Produced by an Unsloth training/export job.
    UnslothOutput,
}

impl std::fmt::Display for ModelSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelSource::HuggingFace => write!(f, "hugging_face"),
            ModelSource::LocalPath => write!(f, "local_path"),
            ModelSource::UnslothOutput => write!(f, "unsloth_output"),
        }
    }
}

/// One downloaded file and how it was checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileVerification {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub verification: Verification,
}

/// Metadata for a model stored locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModel {
    /// Stable local identifier, derived from the repo id and revision.
    pub id: String,
    /// Hugging Face repository id, e.g. `meta-llama/Llama-2-7b-hf`.
    pub repo_id: String,
    /// Git revision or branch that was downloaded.
    pub revision: String,
    /// Quantization suffix selected by the caller, if any.
    pub quantization: Option<String>,
    /// Where the model came from.
    pub source: ModelSource,
    /// Current lifecycle status.
    pub status: ModelStatus,
    /// Total bytes expected across all target files.
    pub total_bytes: u64,
    /// Bytes downloaded so far.
    pub downloaded_bytes: u64,
    /// Local directory containing the downloaded files.
    pub path: PathBuf,
    /// Error message when status is `Failed`.
    pub error_message: Option<String>,
    /// How the downloaded weights were checked (contract `verification`).
    /// The weakest state across the weight files; `None` until a download
    /// or `/models/verify` finishes.
    #[serde(default)]
    pub verification: Option<Verification>,
    /// Per-file result of the last download or verify.
    #[serde(default)]
    pub files: Vec<FileVerification>,
    /// When the record was created.
    pub created_at: DateTime<Utc>,
    /// When the record was last updated.
    pub updated_at: DateTime<Utc>,
}

impl CachedModel {
    /// Create a new pending cached model entry.
    pub fn new(
        id: impl Into<String>,
        repo_id: impl Into<String>,
        revision: impl Into<String>,
        quantization: Option<String>,
        path: PathBuf,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            repo_id: repo_id.into(),
            revision: revision.into(),
            quantization,
            source: ModelSource::HuggingFace,
            status: ModelStatus::Pending,
            total_bytes: 0,
            downloaded_bytes: 0,
            path,
            error_message: None,
            verification: None,
            files: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a pending cached model entry for a local import.
    pub fn new_local(
        id: impl Into<String>,
        name: impl Into<String>,
        path: PathBuf,
        source: ModelSource,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            repo_id: name.into(),
            revision: "local".to_string(),
            quantization: None,
            source,
            status: ModelStatus::Pending,
            total_bytes: 0,
            downloaded_bytes: 0,
            path,
            error_message: None,
            verification: None,
            files: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Progress as a value in the range `[0.0, 1.0]`.
    pub fn progress(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.downloaded_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }

    /// Touch the updated-at timestamp.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    /// Mark the model as actively downloading.
    pub fn set_downloading(&mut self) {
        self.status = ModelStatus::Downloading;
        self.error_message = None;
        self.touch();
    }

    /// Update byte progress.
    pub fn set_progress(&mut self, downloaded: u64, total: u64) {
        self.downloaded_bytes = downloaded;
        self.total_bytes = total;
        self.touch();
    }

    /// Mark the model as ready.
    pub fn set_ready(&mut self) {
        self.status = ModelStatus::Ready;
        self.downloaded_bytes = self.total_bytes;
        self.error_message = None;
        self.touch();
    }

    /// Record how the files were checked.
    pub fn set_verification(&mut self, verification: Verification, files: Vec<FileVerification>) {
        self.verification = Some(verification);
        self.files = files;
        self.touch();
    }

    /// Mark the model as failed with an error message.
    pub fn set_failed(&mut self, message: impl Into<String>) {
        self.status = ModelStatus::Failed;
        self.error_message = Some(message.into());
        self.touch();
    }
}
