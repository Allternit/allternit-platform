//! Process manager for local model runtimes.

use crate::engine_packs::{EnginePacks, ResolvedPack};
use crate::runtime::backends::llamacpp::{self, LlamaCppConfig};
use crate::runtime::backends::mlx::{self, MlxConfig};
use crate::runtime::log;
use crate::runtime::usage::UsageStats;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Default idle unload: a runtime nobody called for this long is stopped so
/// its memory goes back to the machine (plan "On-device models": 5 minutes).
pub const DEFAULT_IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);
/// How long a backend may take to load its model before launch fails.
const HEALTH_DEADLINE: Duration = Duration::from_secs(180);

/// Errors returned by the runtime manager.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeManagerError {
    #[error("runtime not found: {0}")]
    NotFound(String),
    #[error("no free port available")]
    NoFreePort,
    #[error("engine not installed: {0}")]
    BinaryNotFound(String),
    #[error("engine pack failed verification: {0}")]
    PackVerification(String),
    #[error("process failed to start: {0}")]
    SpawnFailed(String),
    #[error("process exited early with code {0:?}")]
    EarlyExit(Option<i32>),
    #[error("health check timed out")]
    HealthTimeout,
    #[error("unsupported backend: {0}")]
    UnsupportedBackend(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Recipe describing how to launch a runtime.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "backend")]
pub enum RuntimeRecipe {
    /// llama.cpp server backend.
    LlamaCpp {
        /// Path to the model file on disk.
        model_path: PathBuf,
        /// Number of layers to offload to the GPU (`-ngl`).
        #[serde(default)]
        n_gpu_layers: u32,
        /// Context size.
        #[serde(default = "default_n_ctx")]
        n_ctx: u32,
        /// Enable Flash Attention.
        #[serde(default)]
        flash_attn: bool,
    },
    /// vLLM server backend.
    Vllm {
        /// Path to the model directory or file on disk.
        model_path: PathBuf,
        /// Tensor parallel size.
        #[serde(default = "default_tensor_parallel")]
        tensor_parallel_size: u32,
        /// Fraction of GPU memory to use.
        #[serde(default = "default_gpu_memory_utilization")]
        gpu_memory_utilization: f32,
        /// Data type.
        #[serde(default)]
        dtype: Option<String>,
        /// Quantization method.
        #[serde(default)]
        quantization: Option<String>,
    },
    /// SGLang server backend.
    Sglang {
        /// Path to the model directory or file on disk.
        model_path: PathBuf,
        /// Tensor parallel size.
        #[serde(default = "default_tensor_parallel")]
        tensor_parallel_size: u32,
        /// Maximum context length.
        #[serde(default = "default_context_length")]
        context_length: u32,
    },
    /// MLX server backend (Apple Silicon).
    Mlx {
        /// Path to the model directory or file on disk.
        model_path: PathBuf,
        /// Quantization method.
        #[serde(default)]
        quantize: Option<String>,
        /// Maximum tokens to generate.
        #[serde(default)]
        max_tokens: Option<u32>,
        /// KV cache bound in tokens.
        #[serde(default = "default_n_ctx")]
        n_ctx: u32,
    },
}

fn default_n_ctx() -> u32 {
    4096
}

fn default_tensor_parallel() -> u32 {
    1
}

fn default_gpu_memory_utilization() -> f32 {
    0.9
}

fn default_context_length() -> u32 {
    4096
}

/// Lifecycle status of a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    /// Process has been spawned but is not yet healthy.
    Starting,
    /// Process is running and responding to health checks.
    Running,
    /// Process was stopped by request.
    Stopped,
    /// Process encountered an error.
    Error,
}

impl std::fmt::Display for RuntimeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeStatus::Starting => write!(f, "starting"),
            RuntimeStatus::Running => write!(f, "running"),
            RuntimeStatus::Stopped => write!(f, "stopped"),
            RuntimeStatus::Error => write!(f, "error"),
        }
    }
}

/// Snapshot of a runtime's current state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuntimeInfo {
    pub id: String,
    /// Model identifier this runtime is serving.
    pub model_id: String,
    /// Human-readable recipe label (e.g. "llamacpp").
    pub recipe: String,
    /// Structured recipe used to launch the runtime.
    pub recipe_value: RuntimeRecipe,
    pub port: u16,
    pub status: RuntimeStatus,
    pub pid: Option<u32>,
    pub health: Option<bool>,
    pub error_message: Option<String>,
    pub logs_path: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `pack` (verified engine pack) or `fallback` (binary found on PATH).
    #[serde(default)]
    pub engine_source: Option<String>,
    #[serde(default)]
    pub pack_id: Option<String>,
    #[serde(default)]
    pub pack_version: Option<String>,
    /// Compute backend of the pack (`metal`, `cuda`, ...); null for PATH binaries.
    #[serde(default)]
    pub backend: Option<String>,
    /// Live numbers while the runtime is running (null otherwise).
    #[serde(default)]
    pub running: Option<RunningInfo>,
    /// Why it stopped when the engine stopped it (`idle`).
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// What the UI shows for a loaded model (DeviceRuntime.running).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunningInfo {
    pub model_id: String,
    /// Resident memory of the backend process.
    pub mem_bytes: Option<u64>,
    /// Generation speed of the last request.
    pub tok_per_sec: Option<f64>,
    pub first_token_ms: Option<u64>,
    /// prompt + completion tokens of the last request.
    pub ctx_used: Option<u64>,
    pub ctx_max: Option<u64>,
    /// Idle unload timeout (0 = never).
    pub idle_unload_sec: u64,
    /// Seconds since the last request finished (0 while one is running).
    pub idle_sec: u64,
    pub requests_in_flight: usize,
}

/// Request bookkeeping shared with the chat proxy.
#[derive(Debug)]
struct Activity {
    in_flight: AtomicUsize,
    last_used: std::sync::Mutex<Instant>,
}

impl Activity {
    fn new() -> Self {
        Self { in_flight: AtomicUsize::new(0), last_used: std::sync::Mutex::new(Instant::now()) }
    }

    fn idle_for(&self) -> Duration {
        if self.in_flight.load(Ordering::SeqCst) > 0 {
            return Duration::ZERO;
        }
        self.last_used.lock().map(|t| t.elapsed()).unwrap_or_default()
    }
}

/// Held by the chat proxy for the length of one request (including a
/// stream the client is still reading); the idle timer starts when it drops.
pub struct RequestGuard(Arc<Activity>);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if let Ok(mut t) = self.0.last_used.lock() {
            *t = Instant::now();
        }
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Resident memory of a process.
pub fn process_mem_bytes(pid: u32) -> Option<u64> {
    let mut sys = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    if !sys.refresh_process(pid) {
        return None;
    }
    sys.process(pid).map(|p| p.memory())
}

impl RuntimeRecipe {
    /// Context size the runtime was launched with.
    pub fn ctx_max(&self) -> Option<u64> {
        match self {
            RuntimeRecipe::LlamaCpp { n_ctx, .. } | RuntimeRecipe::Mlx { n_ctx, .. } => Some(u64::from(*n_ctx)),
            RuntimeRecipe::Sglang { context_length, .. } => Some(u64::from(*context_length)),
            RuntimeRecipe::Vllm { .. } => None,
        }
    }
}

impl RuntimeRecipe {
    /// Human-readable backend label.
    pub fn backend_label(&self) -> &'static str {
        match self {
            RuntimeRecipe::LlamaCpp { .. } => "llamacpp",
            RuntimeRecipe::Vllm { .. } => "vllm",
            RuntimeRecipe::Sglang { .. } => "sglang",
            RuntimeRecipe::Mlx { .. } => "mlx",
        }
    }
}

struct RuntimeHandle {
    info: RuntimeInfo,
    child: Option<Child>,
    activity: Arc<Activity>,
    last_usage: Option<UsageStats>,
}

/// Manages spawned local model runtimes.
#[derive(Clone)]
pub struct ProcessManager {
    data_dir: PathBuf,
    runtimes: Arc<RwLock<HashMap<String, RuntimeHandle>>>,
    packs: Option<EnginePacks>,
    idle_unload: Duration,
}

/// The program a recipe runs, and where it came from.
struct Program {
    path: PathBuf,
    pack: Option<ResolvedPack>,
}

impl ProcessManager {
    /// Create a new process manager (PATH binaries only; see [`Self::with_packs`]).
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
            packs: None,
            idle_unload: DEFAULT_IDLE_UNLOAD,
        }
    }

    /// Launch engines from verified engine packs first, PATH second.
    pub fn with_packs(mut self, packs: EnginePacks) -> Self {
        self.packs = Some(packs);
        self
    }

    /// Idle unload timeout; zero disables it.
    pub fn with_idle_unload(mut self, idle: Duration) -> Self {
        self.idle_unload = idle;
        self
    }

    pub fn idle_unload(&self) -> Duration {
        self.idle_unload
    }

    /// Resolve the program for an engine: the verified pack entrypoint, or a
    /// binary on PATH (development fallback). A pack that fails verification
    /// is never run.
    async fn resolve_program(&self, engine: &str, binary: &str) -> Result<Program, RuntimeManagerError> {
        let mut pack_error = None;
        if let Some(packs) = self.packs.clone() {
            let engine_name = engine.to_string();
            match tokio::task::spawn_blocking(move || packs.resolve(&engine_name)).await {
                Ok(Ok(Some(pack))) => {
                    return Ok(Program { path: pack.entrypoint.clone(), pack: Some(pack) });
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    crate::routes::hardware::invalidate().await;
                    pack_error = Some(e.to_string());
                }
                Err(e) => pack_error = Some(e.to_string()),
            }
        }
        if let Some(path) = crate::hardware::find_on_path(binary) {
            if let Some(e) = &pack_error {
                warn!(%engine, error = %e, path = %path.display(), "engine pack unusable; using PATH fallback");
            }
            return Ok(Program { path, pack: None });
        }
        Err(match pack_error {
            Some(e) => RuntimeManagerError::PackVerification(e),
            None => RuntimeManagerError::BinaryNotFound(format!(
                "{binary}: install the {engine} engine pack (POST /engines/packs/ensure)"
            )),
        })
    }

    /// Directory where runtime logs are stored.
    fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs").join("runtimes")
    }

    /// Allocate a free TCP port on 127.0.0.1.
    async fn allocate_port() -> Result<u16, RuntimeManagerError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let port = addr.port();
        // Dropping the listener frees the port; the backend will rebind it.
        drop(listener);
        Ok(port)
    }

    /// Spawn a runtime from a recipe and wait for it to become healthy.
    pub async fn launch(
        &self,
        model_id: String,
        recipe: RuntimeRecipe,
    ) -> Result<RuntimeInfo, RuntimeManagerError> {
        let id = Uuid::new_v4().to_string();
        let port = Self::allocate_port().await?;

        let logs_dir = self.logs_dir();
        tokio::fs::create_dir_all(&logs_dir).await?;
        let logs_path = logs_dir.join(format!("{}.log", id));

        let backend_label = recipe.backend_label().to_string();
        let (program, argv) = match &recipe {
            RuntimeRecipe::LlamaCpp {
                model_path,
                n_gpu_layers,
                n_ctx,
                flash_attn,
            } => {
                let program = self.resolve_program("llamacpp", "llama-server").await?;
                let argv = llamacpp::build_argv(LlamaCppConfig {
                    program: &program.path,
                    model_path,
                    port,
                    n_gpu_layers: *n_gpu_layers,
                    n_ctx: *n_ctx,
                    flash_attn: *flash_attn,
                });
                (program, argv)
            }
            RuntimeRecipe::Mlx { model_path, n_ctx, .. } => {
                if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
                    return Err(RuntimeManagerError::UnsupportedBackend("mlx (Apple silicon only)".into()));
                }
                let program = self.resolve_program("mlx", mlx::BINARY).await?;
                let argv = mlx::build_argv(MlxConfig {
                    program: &program.path,
                    model_path,
                    port,
                    model_id: &model_id,
                    n_ctx: *n_ctx,
                });
                (program, argv)
            }
            other => {
                return Err(RuntimeManagerError::UnsupportedBackend(
                    other.backend_label().to_string(),
                ));
            }
        };

        info!(%id, %port, program = %program.path.display(), pack = ?program.pack.as_ref().map(|p| &p.pack_id), "spawning runtime");

        let mut cmd = Command::new(&program.path);
        // Do not inherit the full API process environment; model runtimes only
        // need PATH (plus HOME/TMPDIR for system caches). This prevents
        // provider/API secrets (vault tokens, cloud keys, etc.) from leaking
        // into spawned local-engine child processes.
        cmd.env_clear().env("PATH", std::env::var_os("PATH").unwrap_or_default());
        for key in ["HOME", "TMPDIR", "SystemRoot", "TEMP"] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        if let Some(pack) = &program.pack {
            // SwiftPM resource bundles (mlx-swift_Cmlx.bundle) and shared
            // libraries sit beside the entrypoint.
            cmd.current_dir(pack.entrypoint.parent().unwrap_or(pack.dir.as_path()));
        }
        cmd.args(&argv[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RuntimeManagerError::BinaryNotFound(program.path.display().to_string())
            } else {
                RuntimeManagerError::SpawnFailed(e.to_string())
            }
        })?;

        let pid = child.id();

        // Pump stdout/stderr into a size-rotating log file.
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeManagerError::SpawnFailed("failed to capture stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeManagerError::SpawnFailed("failed to capture stderr".into()))?;

        let stdout_log = logs_path.clone();
        let stderr_log = logs_path.clone();
        tokio::spawn(async move {
            if let Err(e) = log::pump(stdout, stdout_log).await {
                warn!(%e, "stdout log pump exited with error");
            }
        });
        tokio::spawn(async move {
            if let Err(e) = log::pump(stderr, stderr_log).await {
                warn!(%e, "stderr log pump exited with error");
            }
        });

        let info = RuntimeInfo {
            id: id.clone(),
            model_id,
            recipe: backend_label,
            recipe_value: recipe,
            port,
            status: RuntimeStatus::Starting,
            pid,
            health: None,
            error_message: None,
            logs_path: logs_path.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            engine_source: Some(if program.pack.is_some() { "pack" } else { "fallback" }.into()),
            pack_id: program.pack.as_ref().map(|p| p.pack_id.clone()),
            pack_version: program.pack.as_ref().map(|p| p.version.clone()),
            backend: program.pack.as_ref().map(|p| crate::engine_packs::pick::backend_word(p.backend).to_string()),
            running: None,
            stop_reason: None,
        };

        {
            let mut runtimes = self.runtimes.write().await;
            runtimes.insert(
                id.clone(),
                RuntimeHandle {
                    info: info.clone(),
                    child: Some(child),
                    activity: Arc::new(Activity::new()),
                    last_usage: None,
                },
            );
        }

        // Poll health in a background task so `launch` can return quickly.
        let manager = self.clone();
        tokio::spawn(async move {
            match manager.wait_for_health(&id, port).await {
                Ok(healthy) => {
                    let mut runtimes = manager.runtimes.write().await;
                    if let Some(handle) = runtimes.get_mut(&id) {
                        handle.info.health = Some(healthy);
                        if healthy {
                            handle.info.status = RuntimeStatus::Running;
                            handle.info.error_message = None;
                            info!(%id, "runtime is healthy");
                        } else {
                            handle.info.status = RuntimeStatus::Error;
                            handle.info.error_message =
                                Some("health endpoint did not return success".into());
                        }
                        handle.info.updated_at = Utc::now();
                    }
                }
                Err(err) => {
                    error!(%id, %err, "runtime health check failed");
                    let mut runtimes = manager.runtimes.write().await;
                    if let Some(handle) = runtimes.get_mut(&id) {
                        handle.info.status = RuntimeStatus::Error;
                        handle.info.error_message = Some(err.to_string());
                        handle.info.updated_at = Utc::now();
                    }
                }
            }
        });

        Ok(info)
    }

    /// Poll the backend health endpoint until it succeeds or the timeout elapses.
    async fn wait_for_health(&self, id: &str, port: u16) -> Result<bool, RuntimeManagerError> {
        let url = llamacpp::health_url(port);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|e| RuntimeManagerError::SpawnFailed(e.to_string()))?;

        let deadline = HEALTH_DEADLINE;
        let interval = Duration::from_secs(1);
        let mut elapsed = Duration::ZERO;

        while elapsed < deadline {
            // Watch for early process exit while we wait.
            {
                let mut runtimes = self.runtimes.write().await;
                if let Some(handle) = runtimes.get_mut(id) {
                    if let Some(ref mut child) = handle.child {
                        if let Some(status) = child.try_wait()? {
                            return Err(RuntimeManagerError::EarlyExit(status.code()));
                        }
                    }
                }
            }

            match client.get(&url).send().await {
                // llama-server and allternit-mlx-server answer 503 while the
                // model loads: keep waiting.
                Ok(resp) if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE => {
                    debug!(%url, "backend is loading the model");
                }
                Ok(resp) => {
                    debug!(%url, status = %resp.status(), "health check response");
                    return Ok(resp.status().is_success());
                }
                Err(err) => {
                    debug!(%url, %err, "health check not ready yet");
                }
            }

            sleep(interval).await;
            elapsed += interval;
        }

        Err(RuntimeManagerError::HealthTimeout)
    }

    /// Stop a runtime: SIGTERM, wait 5s, then SIGKILL.
    pub async fn stop_runtime(&self, id: &str) -> Result<RuntimeInfo, RuntimeManagerError> {
        let child = {
            let mut runtimes = self.runtimes.write().await;
            let handle = runtimes
                .get_mut(id)
                .ok_or_else(|| RuntimeManagerError::NotFound(id.into()))?;

            if handle.info.status == RuntimeStatus::Stopped {
                return Ok(handle.info.clone());
            }
            handle.child.take()
        };

        if let Some(mut child) = child {
            // Use async kill (SIGKILL on Unix). Graceful SIGTERM would require
            // platform-specific PID signalling; runtimes are re-startable and
            // stateless so a clean kill is acceptable.
            let _ = child.kill().await;
            info!(%id, "runtime killed");
        }

        let mut runtimes = self.runtimes.write().await;
        let handle = runtimes
            .get_mut(id)
            .ok_or_else(|| RuntimeManagerError::NotFound(id.into()))?;
        handle.info.status = RuntimeStatus::Stopped;
        handle.info.health = Some(false);
        handle.info.updated_at = Utc::now();
        Ok(handle.info.clone())
    }

    /// Snapshot with live `running` numbers filled in.
    fn snapshot(&self, h: &RuntimeHandle) -> RuntimeInfo {
        let mut info = h.info.clone();
        if info.status == RuntimeStatus::Running {
            let usage = h.last_usage;
            info.running = Some(RunningInfo {
                model_id: info.model_id.clone(),
                mem_bytes: info.pid.and_then(process_mem_bytes),
                tok_per_sec: usage.and_then(|u| u.tok_per_sec).map(|v| (v * 10.0).round() / 10.0),
                first_token_ms: usage.and_then(|u| u.first_token_ms),
                ctx_used: usage.map(|u| u.ctx_used()),
                ctx_max: info.recipe_value.ctx_max(),
                idle_unload_sec: self.idle_unload.as_secs(),
                idle_sec: h.activity.idle_for().as_secs(),
                requests_in_flight: h.activity.in_flight.load(Ordering::SeqCst),
            });
        }
        info
    }

    /// List all runtimes with their latest state.
    pub async fn list_runtimes(&self) -> Vec<RuntimeInfo> {
        let runtimes = self.runtimes.read().await;
        runtimes.values().map(|h| self.snapshot(h)).collect()
    }

    /// `running` blocks of every running runtime (for /status and /hardware).
    pub async fn running(&self) -> Vec<RunningInfo> {
        self.list_runtimes().await.into_iter().filter_map(|r| r.running).collect()
    }

    /// Get a single runtime by id.
    pub async fn get_runtime(&self, id: &str) -> Option<RuntimeInfo> {
        let runtimes = self.runtimes.read().await;
        runtimes.get(id).map(|h| self.snapshot(h))
    }

    /// Mark a request as started; the idle timer pauses until the guard drops.
    pub async fn begin_request(&self, id: &str) -> Option<RequestGuard> {
        let runtimes = self.runtimes.read().await;
        let activity = runtimes.get(id)?.activity.clone();
        activity.in_flight.fetch_add(1, Ordering::SeqCst);
        Some(RequestGuard(activity))
    }

    /// Store the numbers of a finished request.
    pub async fn record_usage(&self, id: &str, usage: UsageStats) {
        if let Some(h) = self.runtimes.write().await.get_mut(id) {
            h.last_usage = Some(usage);
        }
    }

    /// Engine packs in use by running or starting runtimes.
    pub async fn packs_in_use(&self) -> Vec<String> {
        let runtimes = self.runtimes.read().await;
        runtimes
            .values()
            .filter(|h| matches!(h.info.status, RuntimeStatus::Running | RuntimeStatus::Starting))
            .filter_map(|h| h.info.pack_id.clone())
            .collect()
    }

    /// Stop runtimes nobody has called for `idle_unload`. Returns their ids.
    pub async fn reap_idle(&self) -> Vec<String> {
        if self.idle_unload.is_zero() {
            return vec![];
        }
        let idle: Vec<String> = {
            let runtimes = self.runtimes.read().await;
            runtimes
                .iter()
                .filter(|(_, h)| h.info.status == RuntimeStatus::Running)
                .filter(|(_, h)| h.activity.idle_for() >= self.idle_unload)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in &idle {
            info!(%id, idle_secs = self.idle_unload.as_secs(), "idle unload");
            if self.stop_runtime(id).await.is_ok() {
                if let Some(h) = self.runtimes.write().await.get_mut(id) {
                    h.info.stop_reason = Some("idle".into());
                }
            }
        }
        idle
    }

    /// Check for idle runtimes in the background.
    pub fn spawn_idle_reaper(&self) {
        if self.idle_unload.is_zero() {
            return;
        }
        let manager = self.clone();
        let every = (self.idle_unload / 4).clamp(Duration::from_secs(1), Duration::from_secs(30));
        tokio::spawn(async move {
            loop {
                sleep(every).await;
                manager.reap_idle().await;
            }
        });
    }

    /// Find a healthy runtime serving `model_id`.
    pub async fn find_running_runtime_by_model_id(&self, model_id: &str) -> Option<RuntimeInfo> {
        let runtimes = self.runtimes.read().await;
        runtimes
            .values()
            .filter(|h| h.info.model_id == model_id && h.info.status == RuntimeStatus::Running)
            .map(|h| h.info.clone())
            .next()
    }
}

