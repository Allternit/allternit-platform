//! In-memory model cache store with persistence helpers.

use crate::cache::model::{CachedModel, FileVerification, ModelStatus};
use allternit_model_source::Verification;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Manages the local model cache metadata.
#[derive(Clone)]
pub struct ModelStore {
    models_dir: PathBuf,
    models: Arc<RwLock<HashMap<String, CachedModel>>>,
}

impl ModelStore {
    /// Open a model store rooted at `models_dir`.
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self {
            models_dir: models_dir.into(),
            models: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Directory where models are stored.
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Derive a safe local directory from a repo id.
    pub fn normalize_repo_id(repo_id: &str) -> String {
        repo_id.replace('/', "--")
    }

    /// Compute the local directory for a given repo.
    ///
    /// Matches the task layout: `{models_dir}/{repo_id_normalized}`.
    pub fn model_path(&self, repo_id: &str) -> PathBuf {
        self.models_dir.join(Self::normalize_repo_id(repo_id))
    }

    /// Generate a stable local id from repo + revision + optional quantization.
    pub fn model_id(repo_id: &str, revision: &str, quantization: Option<&str>) -> String {
        let mut id = format!("{}@{}", repo_id.replace('/', "--"), revision);
        if let Some(q) = quantization {
            id.push(':');
            id.push_str(q);
        }
        id
    }

    /// List all cached models.
    pub async fn list(&self) -> Vec<CachedModel> {
        let models = self.models.read().await;
        models.values().cloned().collect()
    }

    /// Get a single cached model by id.
    pub async fn get(&self, id: &str) -> Option<CachedModel> {
        self.models.read().await.get(id).cloned()
    }

    /// Insert a new cached model record.
    pub async fn insert(&self, model: CachedModel) {
        let mut models = self.models.write().await;
        models.insert(model.id.clone(), model);
    }

    /// Update progress for a model.
    pub async fn update_progress(&self, id: &str, downloaded: u64, total: u64) {
        let mut models = self.models.write().await;
        if let Some(model) = models.get_mut(id) {
            model.set_progress(downloaded, total);
        }
    }

    /// Set a model's status to downloading.
    pub async fn set_downloading(&self, id: &str) {
        let mut models = self.models.write().await;
        if let Some(model) = models.get_mut(id) {
            model.set_downloading();
        }
    }

    /// Mark a model as ready.
    pub async fn set_ready(&self, id: &str) {
        let mut models = self.models.write().await;
        if let Some(model) = models.get_mut(id) {
            model.set_ready();
        }
    }

    /// Record how a model's files were checked.
    pub async fn set_verification(
        &self,
        id: &str,
        verification: Verification,
        files: Vec<FileVerification>,
    ) {
        let mut models = self.models.write().await;
        if let Some(model) = models.get_mut(id) {
            model.set_verification(verification, files);
        }
    }

    /// Ids of records for `repo_id` (any revision / quantization).
    pub async fn ids_for_repo(&self, repo_id: &str) -> Vec<String> {
        let models = self.models.read().await;
        models
            .values()
            .filter(|m| m.repo_id == repo_id)
            .map(|m| m.id.clone())
            .collect()
    }

    /// Mark a model as failed.
    pub async fn set_failed(&self, id: &str, message: impl Into<String>) {
        let mut models = self.models.write().await;
        if let Some(model) = models.get_mut(id) {
            model.set_failed(message);
        }
    }

    /// Count models currently in a given status.
    pub async fn count_by_status(&self, status: ModelStatus) -> usize {
        let models = self.models.read().await;
        models.values().filter(|m| m.status == status).count()
    }
}
