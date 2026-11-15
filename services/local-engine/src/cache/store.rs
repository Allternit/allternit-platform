//! In-memory model cache store with persistence helpers.

use crate::cache::model::{CachedModel, FileVerification, ModelSource, ModelStatus};
use allternit_model_source::Verification;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

/// File (next to `models_dir`, in the service's data dir) that keeps
/// registered Studio outputs across restarts. Downloads are rebuilt from the
/// models folder; a Studio output lives in its job folder, so only this file
/// knows about it.
pub const STUDIO_OUTPUTS_FILE: &str = "studio-outputs.json";

/// A `POST /models/studio-output` record (id `studio--<job>--<kind>`).
fn is_registered_output(m: &CachedModel) -> bool {
    m.source == ModelSource::StudioOutput && m.id.starts_with("studio--")
}

/// Manages the local model cache metadata.
#[derive(Clone)]
pub struct ModelStore {
    models_dir: PathBuf,
    models: Arc<RwLock<HashMap<String, CachedModel>>>,
    /// One writer of [`STUDIO_OUTPUTS_FILE`] at a time.
    persist: Arc<Mutex<()>>,
}

impl ModelStore {
    /// Open a model store rooted at `models_dir`.
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self {
            models_dir: models_dir.into(),
            models: Arc::new(RwLock::new(HashMap::new())),
            persist: Arc::new(Mutex::new(())),
        }
    }

    /// Where registered Studio outputs are saved.
    pub fn studio_outputs_file(&self) -> PathBuf {
        self.models_dir
            .parent()
            .unwrap_or(&self.models_dir)
            .join(STUDIO_OUTPUTS_FILE)
    }

    /// Load the saved Studio outputs (call once at start). Entries whose
    /// files are gone are dropped, and the file is rewritten without them.
    /// Returns how many were loaded.
    pub async fn load_studio_outputs(&self) -> usize {
        let file = self.studio_outputs_file();
        let saved: Vec<CachedModel> = match tokio::fs::read(&file).await {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(list) => list,
                Err(err) => {
                    warn!(file = %file.display(), %err, "studio outputs file is not readable; ignoring it");
                    return 0;
                }
            },
            Err(_) => return 0,
        };
        let total = saved.len();
        let mut loaded = 0;
        {
            let mut models = self.models.write().await;
            for model in saved {
                if !is_registered_output(&model) || !model.path.exists() {
                    continue;
                }
                models.entry(model.id.clone()).or_insert(model);
                loaded += 1;
            }
        }
        if loaded != total {
            if let Err(err) = self.save_studio_outputs().await {
                warn!(file = %file.display(), %err, "could not rewrite studio outputs file");
            }
        }
        loaded
    }

    /// Save every Studio output record (atomic replace).
    pub async fn save_studio_outputs(&self) -> std::io::Result<()> {
        let _one_writer = self.persist.lock().await;
        let mut outputs: Vec<CachedModel> = self
            .models
            .read()
            .await
            .values()
            .filter(|m| is_registered_output(m))
            .cloned()
            .collect();
        outputs.sort_by(|a, b| a.id.cmp(&b.id));
        let file = self.studio_outputs_file();
        if let Some(dir) = file.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let tmp = file.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&outputs).map_err(std::io::Error::other)?;
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, &file).await
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
