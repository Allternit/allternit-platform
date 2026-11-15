//! Runs one job: prepare (fetch base, find dataset), start the engine
//! process, post the spec, poll engine events, stop the engine at the end.
//! Also cancel and the restart reattach.

use super::engines::{self, EngineDirs};
use super::store::EngineProc;
use super::{Active, StudioHost};
use allternit_studio_spec::{Job, JobEvent, JobOutput, JobState};
use async_trait::async_trait;
use serde::Deserialize;
use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ── launching ─────────────────────────────────────────────────────────────

pub struct LaunchRequest {
    pub engine_id: String,
    pub port: u16,
    pub token: String,
    pub python: PathBuf,
    pub job_dir: PathBuf,
    /// Absolute.
    pub output_dir: PathBuf,
    pub log_path: PathBuf,
    /// The hardware id the host detected (`ALLTERNIT_STUDIO_HARDWARE`).
    pub hardware: allternit_studio_spec::Hardware,
}

pub struct Launched {
    pub pid: u32,
    /// The port the engine listens on (normally the requested one).
    pub port: u16,
    /// Our child process, when we spawned it in this allternit-api.
    pub child: Option<tokio::process::Child>,
    /// Anything that must live as long as the engine (test engines).
    pub guard: Option<Box<dyn Any + Send + Sync>>,
}

#[async_trait]
pub trait EngineLauncher: Send + Sync {
    fn installed(&self, engine_id: &str, dirs: &EngineDirs) -> bool;
    async fn launch(&self, req: LaunchRequest) -> Result<Launched, String>;
}

/// Production launcher: the engine's venv python, or the developer override
/// command (see [`engines`] module docs for the contract).
pub struct ProcessLauncher;

#[async_trait]
impl EngineLauncher for ProcessLauncher {
    fn installed(&self, engine_id: &str, dirs: &EngineDirs) -> bool {
        engines::override_command(engine_id).is_some()
            || (engines::engine_module(engine_id).is_some() && dirs.venv_installed(engine_id))
    }

    async fn launch(&self, req: LaunchRequest) -> Result<Launched, String> {
        let (program, args): (String, Vec<String>) = match engines::override_command(&req.engine_id)
        {
            Some(cmd) => {
                let mut parts = cmd.split_whitespace().map(|p| {
                    p.replace("{port}", &req.port.to_string())
                        .replace("{token}", &req.token)
                });
                let program = parts.next().ok_or("empty engine command")?;
                (program, parts.collect())
            }
            None => {
                let module = engines::engine_module(&req.engine_id)
                    .ok_or_else(|| format!("{} is not available in this build.", req.engine_id))?;
                (
                    req.python.to_string_lossy().into_owned(),
                    vec![
                        "-m".into(),
                        module.into(),
                        "--port".into(),
                        req.port.to_string(),
                        "--data-dir".into(),
                        req.job_dir.join("engine").to_string_lossy().into_owned(),
                    ],
                )
            }
        };
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&req.log_path)
            .map_err(|e| format!("Could not open the engine log: {e}"))?;
        let log_err = log
            .try_clone()
            .map_err(|e| format!("Could not open the engine log: {e}"))?;
        let mut cmd = tokio::process::Command::new(&program);
        if let Some(src) = engines::tune_source().filter(|_| req.engine_id == "allternit-tune") {
            cmd.env("PYTHONPATH", src);
        }
        // Only when the host has it: an engine never sees an empty value.
        match engines::studio_python() {
            Some(py) => cmd.env("ALLTERNIT_STUDIO_PYTHON", py),
            None => cmd.env_remove("ALLTERNIT_STUDIO_PYTHON"),
        };
        cmd.args(&args)
            .env("ALLTERNIT_TUNE_TOKEN", &req.token)
            .env("ALLTERNIT_ENGINE_TOKEN", &req.token)
            .env("ALLTERNIT_ENGINE_PORT", req.port.to_string())
            .env("ALLTERNIT_STUDIO_JOB_DIR", &req.job_dir)
            .env("ALLTERNIT_STUDIO_OUTPUT_DIR", &req.output_dir)
            .env("ALLTERNIT_STUDIO_HARDWARE", req.hardware.id())
            .current_dir(&req.job_dir)
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log_err)
            // The engine must outlive an allternit-api restart (reattach).
            .kill_on_drop(false);
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd
            .spawn()
            .map_err(|e| format!("Could not start the training engine ({program}): {e}"))?;
        let pid = child.id().ok_or("The training engine exited at once.")?;
        Ok(Launched {
            pid,
            port: req.port,
            child: Some(child),
            guard: None,
        })
    }
}

/// An engine process this allternit-api watches.
pub struct RunningEngine {
    pub proc_: EngineProc,
    pub child: Option<tokio::process::Child>,
    pub guard: Option<Box<dyn Any + Send + Sync>>,
}

// ── process helpers ───────────────────────────────────────────────────────

pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn signal_group(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: plain signal sends. The engine was started as its own
        // process group leader, so -pid reaches its workers too; fall back to
        // the pid alone in case it was not.
        unsafe {
            if libc::killpg(pid as libc::pid_t, sig) != 0 {
                libc::kill(pid as libc::pid_t, sig);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let mut args = vec!["/PID".to_string(), pid.to_string(), "/T".to_string()];
        if force {
            args.push("/F".into());
        }
        let _ = std::process::Command::new("taskkill").args(args).output();
    }
}

/// SIGTERM the engine's process group, wait `grace`, then SIGKILL.
async fn kill_engine(mut eng: RunningEngine, grace: Duration) {
    let pid = eng.proc_.pid;
    signal_group(pid, false);
    let deadline = Instant::now() + grace;
    loop {
        let gone = match eng.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(Some(_))),
            None => !pid_alive(pid),
        };
        if gone {
            break;
        }
        if Instant::now() >= deadline {
            signal_group(pid, true);
            if let Some(c) = eng.child.as_mut() {
                let _ = tokio::time::timeout(Duration::from_secs(2), c.wait()).await;
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(eng.guard.take());
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("No free loopback port for the engine: {e}"))
}

fn new_token() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

// ── engine API v1 client ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct EventsPage {
    state: JobState,
    events: Vec<JobEvent>,
}

async fn engine_get<T: for<'de> Deserialize<'de>>(
    host: &StudioHost,
    p: &EngineProc,
    path: &str,
) -> Result<T, String> {
    let res = host
        .0
        .http
        .get(format!("{}{}", p.base_url(), path))
        .bearer_auth(&p.token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("engine answered {}", res.status()));
    }
    res.json::<T>().await.map_err(|e| e.to_string())
}

async fn engine_answers(host: &StudioHost, p: &EngineProc) -> bool {
    engine_get::<serde_json::Value>(host, p, "/v1/engine")
        .await
        .is_ok()
}

// ── the job ───────────────────────────────────────────────────────────────

fn active(host: &StudioHost, id: &str) -> Arc<Active> {
    host.0
        .active
        .lock()
        .unwrap()
        .entry(id.to_string())
        .or_default()
        .clone()
}

fn engine_proc(host: &StudioHost, id: &str) -> Option<EngineProc> {
    active(host, id)
        .engine
        .lock()
        .unwrap()
        .as_ref()
        .map(|e| e.proc_.clone())
}

fn update_proc(host: &StudioHost, id: &str, f: impl FnOnce(&mut EngineProc)) {
    let a = active(host, id);
    let mut slot = a.engine.lock().unwrap();
    if let Some(eng) = slot.as_mut() {
        f(&mut eng.proc_);
        if let Err(e) = host.0.store.save_engine(id, &eng.proc_) {
            tracing::warn!(job = %id, "studio: could not write engine.json: {e}");
        }
    }
}

pub(crate) async fn run_job(host: StudioHost, id: String) {
    match prepare_and_start(&host, &id).await {
        Ok(()) => poll(host, id).await,
        Err(msg) => {
            // Stop the engine before the job reads as finished, so a finished
            // job never has a live engine behind it.
            stop_job(&host, &id, false).await;
            host.fail(&id, &msg);
        }
    }
}

async fn prepare_and_start(host: &StudioHost, id: &str) -> Result<(), String> {
    let job = host.job(id).ok_or("job vanished")?;
    host.set_state(id, JobState::Preparing);

    let mut spec = job.spec.clone();
    let studio = host.0.cfg.dir.clone();

    match spec.base.path.clone() {
        // A local base: never fetched, handed to the engine as an absolute folder.
        Some(p) => {
            let base_dir = local_base(&p)?;
            host.push_event(
                id,
                "log",
                &format!("Using local base model {}", base_dir.display()),
                None,
            );
            spec.base.path = Some(base_dir.to_string_lossy().into_owned());
        }
        None => {
            host.push_event(
                id,
                "log",
                &format!("Getting base model {}", spec.base.repo),
                None,
            );
            let base_dir = fetch_repo(
                &host.model_source()?,
                &studio.join("models"),
                &spec.base.repo,
                spec.base.revision.as_deref(),
                true,
                spec.base.manifest_sha256.as_deref(),
            )
            .await?;
            host.push_event(
                id,
                "log",
                &format!("Base model ready at {}", base_dir.display()),
                None,
            );
            spec.base.repo = base_dir.to_string_lossy().into_owned();
        }
    }

    let dataset = match (&spec.dataset.path, &spec.dataset.repo) {
        (Some(p), _) => local_dataset(p)?,
        (None, Some(repo)) => {
            host.push_event(id, "log", &format!("Getting dataset {repo}"), None);
            fetch_repo(
                &host.model_source()?,
                &studio.join("datasets"),
                repo,
                spec.dataset.revision.as_deref(),
                false,
                None,
            )
            .await?
        }
        (None, None) => return Err("The job has no dataset.".into()),
    };
    spec.dataset.path = Some(dataset.to_string_lossy().into_owned());
    spec.dataset.repo = None;

    if host.state(id).is_none_or(JobState::is_terminal) {
        return Ok(());
    }

    let job_dir = host.0.store.job_dir(id);
    let output_dir = host.0.store.output_dir(id);
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("Could not create {}: {e}", output_dir.display()))?;
    // Engines report absolute output paths inside this folder, so it is absolute too.
    let output_dir = std::path::absolute(&output_dir)
        .map_err(|e| format!("Could not resolve {}: {e}", output_dir.display()))?;
    let token = new_token();
    let launched = host
        .0
        .cfg
        .launcher
        .launch(LaunchRequest {
            engine_id: spec.engine.clone(),
            port: free_port()?,
            token: token.clone(),
            python: host.0.engines.python(&spec.engine),
            job_dir: job_dir.clone(),
            output_dir,
            log_path: job_dir.join("engine.log"),
            hardware: host.0.cfg.hardware.hardware,
        })
        .await?;
    let proc_ = EngineProc {
        engine: spec.engine.clone(),
        pid: launched.pid,
        port: launched.port,
        token,
        engine_job_id: None,
        engine_seq: 0,
    };
    host.0
        .store
        .save_engine(id, &proc_)
        .map_err(|e| format!("Could not record the engine process: {e}"))?;
    *active(host, id).engine.lock().unwrap() = Some(RunningEngine {
        proc_: proc_.clone(),
        child: launched.child,
        guard: launched.guard,
    });
    host.push_event(
        id,
        "log",
        &format!("Engine {} started (pid {})", spec.engine, proc_.pid),
        None,
    );

    wait_ready(host, id, &proc_).await?;

    let res = host
        .0
        .http
        .post(format!("{}/v1/jobs", proc_.base_url()))
        .bearer_auth(&proc_.token)
        .json(&spec)
        .send()
        .await
        .map_err(|e| format!("Could not hand the job to the engine: {e}"))?;
    let status = res.status();
    if !status.is_success() {
        let body: serde_json::Value = res.json().await.unwrap_or_default();
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("no reason given");
        return Err(format!("The engine refused the job: {msg}"));
    }
    let engine_job: Job = res
        .json()
        .await
        .map_err(|e| format!("The engine gave an unreadable answer: {e}"))?;
    update_proc(host, id, |p| p.engine_job_id = Some(engine_job.id.clone()));
    Ok(())
}

async fn wait_ready(host: &StudioHost, id: &str, p: &EngineProc) -> Result<(), String> {
    let deadline = Instant::now() + host.0.cfg.start_timeout;
    loop {
        if engine_answers(host, p).await {
            return Ok(());
        }
        let exited = {
            let a = active(host, id);
            let mut slot = a.engine.lock().unwrap();
            match slot.as_mut().and_then(|e| e.child.as_mut()) {
                Some(c) => matches!(c.try_wait(), Ok(Some(_))),
                None => !pid_alive(p.pid),
            }
        };
        if exited {
            return Err(format!(
                "The training engine stopped while starting. {}",
                log_tail(&host.0.store.job_dir(id).join("engine.log"))
            ));
        }
        if Instant::now() >= deadline {
            return Err("The training engine did not start in time.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Copy engine events and follow its state until the job finishes.
async fn poll(host: StudioHost, id: String) {
    let mut misses = 0u32;
    loop {
        tokio::time::sleep(host.0.cfg.poll_interval).await;
        if host.state(&id).is_none_or(JobState::is_terminal) {
            return;
        }
        let Some(p) = engine_proc(&host, &id) else {
            return;
        };
        let Some(ejid) = p.engine_job_id.clone() else {
            return;
        };
        let page: EventsPage = match engine_get(
            &host,
            &p,
            &format!("/v1/jobs/{ejid}/events?after={}", p.engine_seq),
        )
        .await
        {
            Ok(page) => {
                misses = 0;
                page
            }
            Err(e) => {
                misses += 1;
                let alive = process_alive(&host, &id, p.pid);
                if !alive || misses >= 20 {
                    tracing::warn!(job = %id, "studio: engine lost: {e}");
                    let tail = log_tail(&host.0.store.job_dir(&id).join("engine.log"));
                    stop_job(&host, &id, false).await;
                    host.fail(
                        &id,
                        &format!("The training engine stopped unexpectedly. {tail}"),
                    );
                    return;
                }
                continue;
            }
        };
        if host.state(&id).is_none_or(JobState::is_terminal) {
            return;
        }
        let mut max_seq = p.engine_seq;
        for ev in &page.events {
            max_seq = max_seq.max(ev.seq);
            // The host writes its own state events when it moves.
            if ev.kind != "state" {
                host.push_event(&id, &ev.kind, &ev.message, ev.progress);
            }
        }
        if max_seq != p.engine_seq {
            update_proc(&host, &id, |pp| pp.engine_seq = max_seq);
        }
        match page.state {
            JobState::Queued | JobState::Preparing => {}
            JobState::Running => advance_to(&host, &id, JobState::Running),
            JobState::Exporting => advance_to(&host, &id, JobState::Exporting),
            JobState::Succeeded => {
                let engine_job: Option<Job> = engine_get(&host, &p, &format!("/v1/jobs/{ejid}"))
                    .await
                    .ok();
                let out_dir = host.0.store.output_dir(&id);
                let outputs = engine_job
                    .map(|j| j.outputs)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|o| JobOutput {
                        path: resolve_output(&out_dir, &o.path),
                        ..o
                    })
                    .collect();
                host.set_outputs(&id, outputs);
                advance_to(&host, &id, JobState::Exporting);
                stop_job(&host, &id, false).await;
                advance_to(&host, &id, JobState::Succeeded);
                return;
            }
            JobState::Failed => {
                let engine_job: Option<Job> = engine_get(&host, &p, &format!("/v1/jobs/{ejid}"))
                    .await
                    .ok();
                let why = engine_job
                    .and_then(|j| j.error)
                    .unwrap_or_else(|| "no reason given".into());
                stop_job(&host, &id, false).await;
                host.fail(&id, &format!("Training failed: {why}"));
                return;
            }
            JobState::Cancelled => {
                stop_job(&host, &id, false).await;
                host.set_state(&id, JobState::Cancelled);
                return;
            }
        }
    }
}

/// Walk forward through the states up to `target`, one event per step.
fn advance_to(host: &StudioHost, id: &str, target: JobState) {
    const ORDER: [JobState; 5] = [
        JobState::Queued,
        JobState::Preparing,
        JobState::Running,
        JobState::Exporting,
        JobState::Succeeded,
    ];
    let Some(cur) = host.state(id) else { return };
    let (Some(from), Some(to)) = (
        ORDER.iter().position(|s| *s == cur),
        ORDER.iter().position(|s| *s == target),
    ) else {
        return;
    };
    for s in &ORDER[from + 1..=to.max(from)] {
        host.set_state(id, *s);
    }
}

fn process_alive(host: &StudioHost, id: &str, pid: u32) -> bool {
    let a = active(host, id);
    let mut slot = a.engine.lock().unwrap();
    match slot.as_mut().and_then(|e| e.child.as_mut()) {
        Some(c) => matches!(c.try_wait(), Ok(None)),
        None => pid_alive(pid),
    }
}

/// End the job's runner and engine. `cancel`: also abort the runner task and
/// ask the engine to cancel before the process is stopped.
pub(crate) async fn stop_job(host: &StudioHost, id: &str, cancel: bool) {
    let a = active(host, id);
    if cancel {
        if let Some(task) = a.task.lock().unwrap().take() {
            task.abort();
        }
    }
    let eng = a.engine.lock().unwrap().take();
    if let Some(eng) = eng {
        if cancel {
            if let Some(ejid) = &eng.proc_.engine_job_id {
                let _ = host
                    .0
                    .http
                    .post(format!("{}/v1/jobs/{ejid}/cancel", eng.proc_.base_url()))
                    .bearer_auth(&eng.proc_.token)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await;
            }
        }
        let pid = eng.proc_.pid;
        kill_engine(eng, host.0.cfg.kill_grace).await;
        host.push_event(id, "log", &format!("Engine stopped (pid {pid})"), None);
    }
    host.0.store.remove_engine(id);
    host.0.active.lock().unwrap().remove(id);
}

/// See [`StudioHost::recover`].
pub(crate) async fn recover(host: &StudioHost) {
    for id in host.non_terminal_ids() {
        let Some(p) = host.0.store.load_engine(&id) else {
            host.fail(
                &id,
                "Allternit restarted before the training engine started. Start the job again.",
            );
            continue;
        };
        let alive = pid_alive(p.pid);
        if alive && p.engine_job_id.is_some() && engine_answers(host, &p).await {
            tracing::info!(job = %id, pid = p.pid, "studio: reattached to running engine");
            host.push_event(
                &id,
                "log",
                &format!(
                    "Reattached to the running engine (pid {}) after Allternit restarted",
                    p.pid
                ),
                None,
            );
            let a = active(host, &id);
            *a.engine.lock().unwrap() = Some(RunningEngine {
                proc_: p,
                child: None,
                guard: None,
            });
            let handle = tokio::spawn(poll(host.clone(), id.clone()));
            *a.task.lock().unwrap() = Some(handle);
        } else {
            if alive {
                // A process with that pid exists but is not our engine, or it
                // never got the job: stop it only if it answers with our token.
                if engine_answers(host, &p).await {
                    kill_engine(
                        RunningEngine {
                            proc_: p.clone(),
                            child: None,
                            guard: None,
                        },
                        host.0.cfg.kill_grace,
                    )
                    .await;
                }
            }
            host.fail(
                &id,
                &format!(
                    "The training engine (pid {}) stopped while Allternit was restarting. Start the job again.",
                    p.pid
                ),
            );
            host.0.store.remove_engine(&id);
        }
    }
}

// ── inputs and outputs ────────────────────────────────────────────────────

fn resolve_output(out_dir: &Path, path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        path.to_string()
    } else {
        out_dir.join(p).to_string_lossy().into_owned()
    }
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

/// A local base model folder (`base.path`): absolute (or `~/`), and a folder
/// that exists. Checked when the job is created and again when it starts.
pub(crate) fn local_base(p: &str) -> Result<PathBuf, String> {
    let path = expand_home(p);
    if !path.is_absolute() {
        return Err(format!(
            "The base model path {p} must be a full path (or start with ~/)."
        ));
    }
    if !path.is_dir() {
        return Err(format!(
            "The base model folder {} was not found on this computer.",
            path.display()
        ));
    }
    Ok(path)
}

fn local_dataset(p: &str) -> Result<PathBuf, String> {
    let path = expand_home(p);
    if !path.is_absolute() {
        return Err(format!(
            "The dataset path {p} must be a full path (or start with ~/)."
        ));
    }
    if !path.exists() {
        return Err(format!(
            "The dataset {} was not found on this computer.",
            path.display()
        ));
    }
    Ok(path)
}

/// Weight formats a trainer never reads; skipped when fetching a base model.
fn skip_for_training(path: &str, has_safetensors: bool) -> bool {
    let lower = path.to_ascii_lowercase();
    let skip_ext = [
        ".gguf",
        ".onnx",
        ".onnx_data",
        ".tflite",
        ".h5",
        ".msgpack",
        ".ot",
        ".mlmodel",
    ];
    skip_ext.iter().any(|e| lower.ends_with(e))
        || lower.starts_with("onnx/")
        || lower.starts_with("original/")
        || (has_safetensors && lower.ends_with(".bin") && lower.contains("pytorch_model"))
}

/// File in a fetched folder holding the sha256 of the signed Freeweights
/// manifest its files were checked against (absent for publisher-hash fetches).
pub(crate) const MANIFEST_SHA_FILE: &str = ".studio-manifest-sha256";

/// The cache folder [`fetch_repo`] uses for `repo` at `rev`.
pub(crate) fn repo_dir(root: &Path, repo: &str, rev: Option<&str>) -> PathBuf {
    let rev_dir = rev.unwrap_or("main").replace(['/', '\\'], "_");
    root.join(repo.replace('/', "__")).join(rev_dir)
}

/// sha256 of the signed manifest a finished cache folder was fetched with.
pub(crate) fn cached_manifest_sha(dir: &Path) -> Option<String> {
    if !dir.join(".studio-complete").exists() {
        return None;
    }
    std::fs::read_to_string(dir.join(MANIFEST_SHA_FILE))
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

/// Plain message for a pinned manifest that is not the one Freeweights serves.
pub(crate) fn manifest_mismatch_message(repo: &str, pinned: &str, found: Option<&str>) -> String {
    match found {
        Some(found) => format!(
            "The signed Freeweights manifest for {repo} is not the one this job pins \
             (base.manifest_sha256 {pinned}, manifest sha256 {found})."
        ),
        None => format!(
            "This job pins {repo} to manifest sha256 {pinned}, but {repo} has no signed \
             Freeweights manifest at that revision to check it against."
        ),
    }
}

/// Fetch every file of `repo` at `rev` with model-source `fetch_verified`
/// into `<root>/<org>__<name>/<rev>/` (Decision 5). A finished folder carries
/// `.studio-complete` and is reused.
///
/// `pin` is the job's `base.manifest_sha256`: the folder is reused only when
/// it was fetched with that manifest, and a fresh fetch refuses (before any
/// file downloads) unless the signed manifest it resolved has that sha256.
pub(crate) async fn fetch_repo(
    client: &allternit_model_source::Client,
    root: &Path,
    repo: &str,
    rev: Option<&str>,
    model: bool,
    pin: Option<&str>,
) -> Result<PathBuf, String> {
    let dir = repo_dir(root, repo, rev);
    let marker = dir.join(".studio-complete");
    let pin = pin.map(|p| p.trim().to_ascii_lowercase());
    if marker.exists() {
        match &pin {
            None => return Ok(dir),
            Some(p) if cached_manifest_sha(&dir).as_deref() == Some(p.as_str()) => return Ok(dir),
            // Fetched with another manifest (or none): fetch again under the pin.
            Some(_) => {
                let _ = std::fs::remove_file(&marker);
            }
        }
    }
    let res = client
        .resolve(repo, rev)
        .await
        .map_err(|e| format!("Could not find {repo}: {e}"))?;
    if let Some(p) = &pin {
        if res.manifest_sha256.as_deref() != Some(p.as_str()) {
            return Err(manifest_mismatch_message(
                repo,
                p,
                res.manifest_sha256.as_deref(),
            ));
        }
    }
    let has_st = res.files.keys().any(|f| f.ends_with(".safetensors"));
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let opts = allternit_model_source::FetchOptions::default();
    for path in res.files.keys() {
        if model && skip_for_training(path, has_st) {
            continue;
        }
        let dest = dir.join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        client
            .fetch_verified(&res, path, &dest, &opts)
            .await
            .map_err(|e| format!("Could not download {path} from {repo}: {e}"))?;
    }
    let sha_file = dir.join(MANIFEST_SHA_FILE);
    match &res.manifest_sha256 {
        Some(sha) => std::fs::write(&sha_file, format!("{sha}\n")),
        None => match std::fs::remove_file(&sha_file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
    .map_err(|e| format!("Could not finish the download: {e}"))?;
    std::fs::write(&marker, format!("{}\n", res.revision))
        .map_err(|e| format!("Could not finish the download: {e}"))?;
    Ok(dir)
}

fn log_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().rev().take(5).collect();
    if lines.is_empty() {
        return String::new();
    }
    let tail: Vec<&str> = lines.into_iter().rev().collect();
    format!("Last engine output: {}", tail.join(" | "))
}
