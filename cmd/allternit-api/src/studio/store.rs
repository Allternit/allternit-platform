//! Job files under `<data dir>/studio/jobs/<id>/` (Decision 6).
//!
//! - `job.json`: the [`Job`] record, rewritten atomically on every change.
//! - `events.jsonl`: one [`JobEvent`] per line, appended, seq strictly +1.
//! - `engine.json`: the running engine process (pid, port, token, engine job
//!   id), mode 0600. Present only while an engine runs for the job; on an
//!   allternit-api restart it decides reattach vs failed.
//!
//! Everything is also held in memory; the files are the source of truth on
//! start-up.

use allternit_studio_spec::{Job, JobEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The engine process a job runs in. Written once the process starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineProc {
    pub engine: String,
    pub pid: u32,
    pub port: u16,
    /// Per-launch bearer token for engine API v1.
    pub token: String,
    /// The engine's own id for the job, once `POST /v1/jobs` returned.
    #[serde(default)]
    pub engine_job_id: Option<String>,
    /// Highest engine event seq already copied into the host log.
    #[serde(default)]
    pub engine_seq: u64,
}

impl EngineProc {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

pub struct JobRec {
    pub job: Job,
    pub events: Vec<JobEvent>,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(studio_dir: &Path) -> Self {
        Self {
            root: studio_dir.join("jobs"),
        }
    }

    pub fn job_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn output_dir(&self, id: &str) -> PathBuf {
        self.job_dir(id).join("outputs")
    }

    /// Every job on disk with its events. Unreadable job folders are skipped
    /// with a warning rather than stopping start-up.
    pub fn load_all(&self) -> BTreeMap<String, JobRec> {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return out;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            let job: Job = match std::fs::read(dir.join("job.json"))
                .map_err(|e| e.to_string())
                .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
            {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(dir = %dir.display(), "studio: skipping unreadable job: {e}");
                    continue;
                }
            };
            let events = read_events(&dir.join("events.jsonl"));
            out.insert(job.id.clone(), JobRec { job, events });
        }
        out
    }

    pub fn save_job(&self, job: &Job) -> std::io::Result<()> {
        let dir = self.job_dir(&job.id);
        std::fs::create_dir_all(&dir)?;
        let bytes = serde_json::to_vec_pretty(job).map_err(std::io::Error::other)?;
        write_atomic(&dir.join("job.json"), &bytes, false)
    }

    pub fn append_event(&self, id: &str, event: &JobEvent) -> std::io::Result<()> {
        let dir = self.job_dir(id);
        std::fs::create_dir_all(&dir)?;
        let mut line = serde_json::to_vec(event).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.jsonl"))?;
        f.write_all(&line)
    }

    pub fn save_engine(&self, id: &str, proc_: &EngineProc) -> std::io::Result<()> {
        let dir = self.job_dir(id);
        std::fs::create_dir_all(&dir)?;
        let bytes = serde_json::to_vec_pretty(proc_).map_err(std::io::Error::other)?;
        write_atomic(&dir.join("engine.json"), &bytes, true)
    }

    pub fn load_engine(&self, id: &str) -> Option<EngineProc> {
        let bytes = std::fs::read(self.job_dir(id).join("engine.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn remove_engine(&self, id: &str) {
        let _ = std::fs::remove_file(self.job_dir(id).join("engine.json"));
    }
}

fn read_events(path: &Path) -> Vec<JobEvent> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut events: Vec<JobEvent> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    events.sort_by_key(|e| e.seq);
    events.dedup_by_key(|e| e.seq);
    events
}

fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
