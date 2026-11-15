//! Freeweights Studio host (Phase 3, step 3): `/api/v1/studio` on each
//! computer's allternit-api.
//!
//! - [`routes`]: the host API in `services/studio-spec/openapi/studio-api.yaml`.
//! - [`store`]: jobs as JSON files under `<data dir>/studio/jobs/`.
//! - [`engines`]: install state, hardware fit, venv install, launch contract.
//! - [`runner`]: starts an engine process, speaks engine API v1 over loopback
//!   with a per-launch token, copies its events, cancels, and reattaches after
//!   an allternit-api restart.
//!
//! One job runs per computer at a time.

pub mod engines;
pub mod routes;
pub mod runner;
pub mod store;
// Unix only: the tests use a `sleep` child and signals as the engine process.
#[cfg(all(test, unix))]
mod tests;

use allternit_studio_spec::{
    engines as catalog, ApiError, Job, JobEvent, JobOutput, JobSpec, JobState,
};
use axum::http::StatusCode;
use chrono::Utc;
use engines::{EngineDirs, EngineView, Hardware, InstallRecord};
use runner::{EngineLauncher, RunningEngine};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use store::{JobRec, Store};

pub use routes::router;

pub struct StudioConfig {
    /// `<data dir>/studio`.
    pub dir: PathBuf,
    pub hardware: Hardware,
    pub launcher: Arc<dyn EngineLauncher>,
    /// Base URL of the local-engine service (outputs/register).
    pub local_engine_url: String,
    pub poll_interval: Duration,
    /// How long a new engine process gets to answer `GET /v1/engine`.
    pub start_timeout: Duration,
    /// SIGTERM, then this long, then SIGKILL.
    pub kill_grace: Duration,
    /// Freeweights base URL for base models and manifests; `None` = model-source
    /// default (`ALLTERNIT_FREEWEIGHTS_URL`, else freeweights.allternit.com).
    pub freeweights_url: Option<String>,
}

impl StudioConfig {
    /// Production settings: `<data_dir>/studio`, detected hardware, venv
    /// engines, `LOCAL_ENGINE_URL` (same default as the local-engine proxy).
    pub fn from_data_dir(data_dir: &std::path::Path) -> Self {
        Self {
            dir: data_dir.join("studio"),
            hardware: Hardware::detect(),
            launcher: Arc::new(runner::ProcessLauncher),
            local_engine_url: std::env::var("LOCAL_ENGINE_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:3015".to_string())
                .trim_end_matches('/')
                .to_string(),
            poll_interval: Duration::from_millis(500),
            start_timeout: Duration::from_secs(120),
            kill_grace: Duration::from_secs(5),
            freeweights_url: None,
        }
    }
}

/// An HTTP error: status plus the `{error, message}` body.
#[derive(Debug)]
pub struct HostError(pub StatusCode, pub ApiError);

impl HostError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self(
            status,
            ApiError {
                error: code.into(),
                message: message.into(),
            },
        )
    }
    fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "There is no such job on this computer.",
        )
    }
}

#[derive(Default)]
pub(crate) struct Active {
    pub engine: Mutex<Option<RunningEngine>>,
    pub task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

pub(crate) struct Inner {
    pub cfg: StudioConfig,
    pub store: Store,
    pub engines: EngineDirs,
    pub jobs: Mutex<BTreeMap<String, JobRec>>,
    pub active: Mutex<HashMap<String, Arc<Active>>>,
    pub installing: Mutex<HashSet<String>>,
    pub http: reqwest::Client,
}

/// Cheap to clone; one per allternit-api process.
#[derive(Clone)]
pub struct StudioHost(pub(crate) Arc<Inner>);

impl StudioHost {
    /// Load jobs from disk. Call [`StudioHost::recover`] next.
    pub fn new(cfg: StudioConfig) -> Self {
        let _ = std::fs::create_dir_all(&cfg.dir);
        let store = Store::new(&cfg.dir);
        let jobs = store.load_all();
        let engines = EngineDirs::new(&cfg.dir);
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        Self(Arc::new(Inner {
            cfg,
            store,
            engines,
            jobs: Mutex::new(jobs),
            active: Mutex::default(),
            installing: Mutex::default(),
            http,
        }))
    }

    /// Jobs left `queued`/`preparing`/`running`/`exporting` by an earlier
    /// allternit-api: reattach when the engine process still answers with its
    /// token, else mark failed with a plain message.
    pub async fn recover(&self) {
        runner::recover(self).await
    }

    // ── engines ────────────────────────────────────────────────────────────

    pub fn engine_views(&self) -> Vec<EngineView> {
        catalog::catalog()
            .into_iter()
            .map(|e| self.engine_view(e))
            .collect()
    }

    fn engine_view(&self, mut info: allternit_studio_spec::EngineInfo) -> EngineView {
        let fit = engines::hardware_fit(&info, self.0.cfg.hardware);
        let rec = self.0.engines.read_install(&info.id);
        let installing = self.0.installing.lock().unwrap().contains(&info.id);
        let installed = self.0.cfg.launcher.installed(&info.id, &self.0.engines);
        info.install_state = if installed {
            allternit_studio_spec::InstallState::Installed
        } else if installing {
            allternit_studio_spec::InstallState::Installing
        } else if rec.state == Some(allternit_studio_spec::InstallState::Failed) {
            allternit_studio_spec::InstallState::Failed
        } else {
            allternit_studio_spec::InstallState::NotInstalled
        };
        let available = engines::engine_module(&info.id).is_some();
        let install_message = match info.install_state {
            allternit_studio_spec::InstallState::Failed => rec.message,
            _ if !available => Some(format!("{} is not available in this build.", info.name)),
            _ => None,
        };
        EngineView {
            info,
            fit,
            available,
            install_message,
        }
    }

    /// Start installing an engine in the background (idempotent).
    pub fn install(&self, id: &str) -> Result<EngineView, HostError> {
        let info = catalog::find(id).ok_or_else(|| {
            HostError::new(
                StatusCode::NOT_FOUND,
                "unknown_engine",
                format!("There is no engine called '{id}'."),
            )
        })?;
        if engines::engine_module(id).is_none() {
            return Err(HostError::new(
                StatusCode::NOT_IMPLEMENTED,
                "not_available",
                format!("{} is not available in this build of Allternit.", info.name),
            ));
        }
        let fit = engines::hardware_fit(&info, self.0.cfg.hardware);
        if !fit.ok {
            return Err(HostError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "hardware_not_supported",
                fit.reason,
            ));
        }
        let view = self.engine_view(info.clone());
        if view.info.install_state == allternit_studio_spec::InstallState::Installed {
            return Ok(view);
        }
        if !self.0.installing.lock().unwrap().insert(id.to_string()) {
            return Ok(view);
        }
        self.0.engines.write_install(
            id,
            &InstallRecord {
                state: Some(allternit_studio_spec::InstallState::Installing),
                message: None,
            },
        );
        let host = self.clone();
        let id_owned = id.to_string();
        tokio::spawn(async move {
            let result = host.0.engines.install(&id_owned).await;
            let rec = match result {
                Ok(()) => InstallRecord {
                    state: Some(allternit_studio_spec::InstallState::Installed),
                    message: None,
                },
                Err(msg) => {
                    tracing::warn!(engine = %id_owned, "studio: engine install failed: {msg}");
                    InstallRecord {
                        state: Some(allternit_studio_spec::InstallState::Failed),
                        message: Some(msg),
                    }
                }
            };
            host.0.engines.write_install(&id_owned, &rec);
            host.0.installing.lock().unwrap().remove(&id_owned);
        });
        Ok(self.engine_view(info))
    }

    /// Freeweights client for base models, datasets and manifests.
    pub(crate) fn model_source(&self) -> Result<allternit_model_source::Client, String> {
        match &self.0.cfg.freeweights_url {
            Some(url) => allternit_model_source::Client::with_urls(
                url.as_str(),
                allternit_model_source::DEFAULT_PUBLISHER_URL,
            ),
            None => allternit_model_source::Client::new(),
        }
        .map_err(|e| format!("Could not reach Freeweights: {e}"))
    }

    /// `base.manifest_sha256` must be the sha256 of the signed Freeweights
    /// manifest the base model is (or will be) fetched with. Checked before
    /// the job is accepted: against the cached fetch when it was made with
    /// that manifest, else against the manifest Freeweights serves now. The
    /// runner checks again when it fetches (see [`runner::fetch_repo`]).
    pub(crate) async fn check_manifest_pin(&self, spec: &JobSpec) -> Result<(), HostError> {
        let Some(pin) = spec.base.manifest_sha256.as_deref() else {
            return Ok(());
        };
        let pin = pin.trim().to_ascii_lowercase();
        let repo = spec.base.repo.as_str();
        let mismatch = |message: String| {
            HostError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "manifest_mismatch",
                message,
            )
        };
        if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(mismatch(format!(
                "base.manifest_sha256 must be a sha256 (64 hex characters); got '{pin}'."
            )));
        }
        let rev = spec.base.revision.as_deref();
        let cached = runner::repo_dir(&self.0.cfg.dir.join("models"), repo, rev);
        if runner::cached_manifest_sha(&cached).as_deref() == Some(pin.as_str()) {
            return Ok(());
        }
        let client = self.model_source().map_err(&mismatch)?;
        let found = match client.fetch_manifest_sha(repo).await {
            Ok(Ok((m, sha))) => {
                // resolve() only uses the signed manifest at its pinned commit.
                let commit = m.upstream.commit.as_str();
                rev.is_none_or(|r| r == "main" || r == commit)
                    .then_some(sha)
            }
            Ok(Err(reason)) => {
                return Err(mismatch(format!(
                    "Could not check base.manifest_sha256 for {repo}: {reason}."
                )))
            }
            Err(e) => {
                return Err(mismatch(format!(
                    "The Freeweights manifest for {repo} failed verification: {e}."
                )))
            }
        };
        if found.as_deref() == Some(pin.as_str()) {
            Ok(())
        } else {
            Err(mismatch(runner::manifest_mismatch_message(
                repo,
                &pin,
                found.as_deref(),
            )))
        }
    }

    // ── jobs ───────────────────────────────────────────────────────────────

    pub async fn create(&self, body: &str) -> Result<Job, HostError> {
        let spec = JobSpec::parse(body).map_err(|errs| {
            let message = errs
                .iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join("; ");
            HostError::new(StatusCode::UNPROCESSABLE_ENTITY, &errs[0].code, message)
        })?;
        let info = catalog::find(&spec.engine).expect("validated engine is in the catalog");
        let fit = engines::hardware_fit(&info, self.0.cfg.hardware);
        if !fit.ok {
            return Err(HostError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "hardware_not_supported",
                fit.reason,
            ));
        }
        if !self.0.cfg.launcher.installed(&info.id, &self.0.engines) {
            return Err(HostError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "engine_not_installed",
                format!(
                    "{} is not installed on this computer yet. Install it first.",
                    info.name
                ),
            ));
        }
        self.check_manifest_pin(&spec).await?;
        let job = {
            let mut jobs = self.0.jobs.lock().unwrap();
            if let Some(busy) = jobs.values().find(|r| !r.job.state.is_terminal()) {
                return Err(HostError::new(
                    StatusCode::CONFLICT,
                    "job_running",
                    format!("Job {} is still running on this computer. Wait for it to finish or cancel it.", busy.job.id),
                ));
            }
            let now = Utc::now();
            let id = format!(
                "job_{}_{}",
                now.format("%Y%m%d%H%M%S"),
                &hex::encode(rand::random::<[u8; 3]>())
            );
            let job = Job {
                id: id.clone(),
                state: JobState::Queued,
                spec,
                created_at: now,
                updated_at: Some(now),
                error: None,
                last_seq: 0,
                outputs: vec![],
            };
            jobs.insert(
                id,
                JobRec {
                    job: job.clone(),
                    events: vec![],
                },
            );
            job
        };
        self.persist_job(&job.id);
        self.push_event(&job.id, "state", "queued", Some(0.0));
        let active = Arc::new(Active::default());
        self.0
            .active
            .lock()
            .unwrap()
            .insert(job.id.clone(), active.clone());
        let handle = tokio::spawn(runner::run_job(self.clone(), job.id.clone()));
        *active.task.lock().unwrap() = Some(handle);
        Ok(self.job(&job.id).unwrap_or(job))
    }

    pub fn jobs(&self) -> Vec<Job> {
        let mut out: Vec<Job> = self
            .0
            .jobs
            .lock()
            .unwrap()
            .values()
            .map(|r| r.job.clone())
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        self.0.jobs.lock().unwrap().get(id).map(|r| r.job.clone())
    }

    pub fn events(&self, id: &str, after: u64) -> Result<(JobState, Vec<JobEvent>), HostError> {
        let jobs = self.0.jobs.lock().unwrap();
        let rec = jobs.get(id).ok_or_else(HostError::not_found)?;
        Ok((
            rec.job.state,
            rec.events
                .iter()
                .filter(|e| e.seq > after)
                .cloned()
                .collect(),
        ))
    }

    pub fn outputs(&self, id: &str) -> Result<Vec<JobOutput>, HostError> {
        self.job(id)
            .map(|j| j.outputs)
            .ok_or_else(HostError::not_found)
    }

    pub async fn cancel(&self, id: &str) -> Result<Job, HostError> {
        let job = self.job(id).ok_or_else(HostError::not_found)?;
        if job.state.is_terminal() {
            return Err(HostError::new(
                StatusCode::CONFLICT,
                "already_finished",
                format!("The job already finished ({}).", state_word(job.state)),
            ));
        }
        // Runner stopped and engine gone first; then the job reads cancelled,
        // so the computer is free only once the GPU is.
        runner::stop_job(self, id, true).await;
        self.set_state(id, JobState::Cancelled);
        Ok(self.job(id).expect("job exists"))
    }

    // ── state helpers (shared by runner) ───────────────────────────────────

    pub(crate) fn persist_job(&self, id: &str) {
        let job = self.job(id);
        if let Some(job) = job {
            if let Err(e) = self.0.store.save_job(&job) {
                tracing::warn!(job = %id, "studio: could not write job.json: {e}");
            }
        }
    }

    /// Append one event with the next seq. Ignored once the job finished,
    /// except for the event that records the finish itself.
    pub(crate) fn push_event(&self, id: &str, kind: &str, message: &str, progress: Option<f64>) {
        let event = {
            let mut jobs = self.0.jobs.lock().unwrap();
            let Some(rec) = jobs.get_mut(id) else { return };
            rec.job.last_seq += 1;
            rec.job.updated_at = Some(Utc::now());
            let event = JobEvent {
                seq: rec.job.last_seq,
                at: Utc::now(),
                kind: kind.into(),
                message: message.into(),
                progress,
            };
            rec.events.push(event.clone());
            event
        };
        if let Err(e) = self.0.store.append_event(id, &event) {
            tracing::warn!(job = %id, "studio: could not write events: {e}");
        }
        self.persist_job(id);
    }

    /// Forward-only transition with its `state` event. False when refused
    /// (already there, or the job finished).
    pub(crate) fn set_state(&self, id: &str, to: JobState) -> bool {
        {
            let mut jobs = self.0.jobs.lock().unwrap();
            let Some(rec) = jobs.get_mut(id) else {
                return false;
            };
            match rec.job.state.transition(to) {
                Ok(s) => rec.job.state = s,
                Err(_) => return false,
            }
        }
        let progress = match to {
            JobState::Succeeded => Some(1.0),
            _ => None,
        };
        self.push_event(id, "state", state_word(to), progress);
        true
    }

    /// Mark failed with a plain message (no-op when already finished).
    pub(crate) fn fail(&self, id: &str, message: &str) {
        let changed = {
            let mut jobs = self.0.jobs.lock().unwrap();
            match jobs.get_mut(id) {
                Some(rec) if !rec.job.state.is_terminal() => {
                    rec.job.error = Some(message.to_string());
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.push_event(id, "error", message, None);
            self.set_state(id, JobState::Failed);
        }
    }

    pub(crate) fn set_outputs(&self, id: &str, outputs: Vec<JobOutput>) {
        if let Some(rec) = self.0.jobs.lock().unwrap().get_mut(id) {
            rec.job.outputs = outputs;
        }
        self.persist_job(id);
    }

    pub(crate) fn state(&self, id: &str) -> Option<JobState> {
        self.0.jobs.lock().unwrap().get(id).map(|r| r.job.state)
    }

    pub(crate) fn non_terminal_ids(&self) -> Vec<String> {
        self.0
            .jobs
            .lock()
            .unwrap()
            .values()
            .filter(|r| !r.job.state.is_terminal())
            .map(|r| r.job.id.clone())
            .collect()
    }

    /// Register a finished job's output as a local model through the
    /// local-engine service (model source `studio_output`).
    pub async fn register_output(
        &self,
        id: &str,
        kind: &str,
        name: Option<String>,
    ) -> Result<String, HostError> {
        let job = self.job(id).ok_or_else(HostError::not_found)?;
        if job.state != JobState::Succeeded {
            return Err(HostError::new(
                StatusCode::CONFLICT,
                "job_not_succeeded",
                format!(
                    "Only a finished job's outputs can be added. This job is {}.",
                    state_word(job.state)
                ),
            ));
        }
        let output = job.outputs.iter().find(|o| o.kind == kind).ok_or_else(|| {
            HostError::new(
                StatusCode::NOT_FOUND,
                "no_output",
                format!("This job has no {kind} output."),
            )
        })?;
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            format!(
                "{}-{}-{}",
                short_repo(&job.spec.base.repo),
                job.spec.method,
                kind
            )
        });
        let url = format!("{}/models/studio-output", self.0.cfg.local_engine_url);
        let body =
            serde_json::json!({ "path": output.path, "name": name, "job_id": id, "kind": kind });
        let res = self.0.http.post(&url).json(&body).send().await.map_err(|e| {
            tracing::warn!("studio: local-engine unreachable: {e}");
            HostError::new(
                StatusCode::BAD_GATEWAY,
                "local_engine_unreachable",
                "The local model engine is not running on this computer, so the output could not be added.",
            )
        })?;
        let status = res.status();
        let value: serde_json::Value = res.json().await.unwrap_or_default();
        if !status.is_success() {
            let msg = value
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(HostError::new(
                StatusCode::BAD_GATEWAY,
                "register_failed",
                format!("The local model engine refused the output: {msg}"),
            ));
        }
        value
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                HostError::new(
                    StatusCode::BAD_GATEWAY,
                    "register_failed",
                    "The local model engine gave no model id.",
                )
            })
    }
}

pub(crate) fn state_word(s: JobState) -> &'static str {
    match s {
        JobState::Queued => "queued",
        JobState::Preparing => "preparing",
        JobState::Running => "running",
        JobState::Exporting => "exporting",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

fn short_repo(repo: &str) -> String {
    repo.rsplit('/').next().unwrap_or(repo).to_string()
}
