//! Process manager for local model runtimes.

use crate::runtime::backends::llamacpp::{self, LlamaCppConfig};
use crate::runtime::log;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Errors returned by the runtime manager.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeManagerError {
    #[error("runtime not found: {0}")]
    NotFound(String),
    #[error("no free port available")]
    NoFreePort,
    #[error("binary not on PATH: {0}")]
    BinaryNotFound(String),
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
}

/// Manages spawned local model runtimes.
#[derive(Clone)]
pub struct ProcessManager {
    data_dir: PathBuf,
    runtimes: Arc<RwLock<HashMap<String, RuntimeHandle>>>,
}

impl ProcessManager {
    /// Create a new process manager.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
        }
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
                if which::which("llama-server").is_none() {
                    return Err(RuntimeManagerError::BinaryNotFound("llama-server".into()));
                }
                let cfg = LlamaCppConfig {
                    model_path,
                    port,
                    n_gpu_layers: *n_gpu_layers,
                    n_ctx: *n_ctx,
                    flash_attn: *flash_attn,
                };
                let argv = llamacpp::build_argv(cfg);
                (argv[0].clone(), argv)
            }
            other => {
                return Err(RuntimeManagerError::UnsupportedBackend(
                    other.backend_label().to_string(),
                ));
            }
        };

        info!(%id, %port, program, "spawning runtime");

        let mut cmd = Command::new(&program);
        // Do not inherit the full API process environment; model runtimes only
        // need PATH to locate their own binaries. This prevents provider/API
        // secrets (vault tokens, cloud keys, etc.) from leaking into spawned
        // local-engine child processes.
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .args(&argv[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RuntimeManagerError::BinaryNotFound(program.clone())
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
        };

        {
            let mut runtimes = self.runtimes.write().await;
            runtimes.insert(
                id.clone(),
                RuntimeHandle {
                    info: info.clone(),
                    child: Some(child),
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

        let deadline = Duration::from_secs(30);
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

    /// List all runtimes with their latest state.
    pub async fn list_runtimes(&self) -> Vec<RuntimeInfo> {
        let runtimes = self.runtimes.read().await;
        runtimes.values().map(|h| h.info.clone()).collect()
    }

    /// Get a single runtime by id.
    pub async fn get_runtime(&self, id: &str) -> Option<RuntimeInfo> {
        let runtimes = self.runtimes.read().await;
        runtimes.get(id).map(|h| h.info.clone())
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

/// Check whether a program is on PATH.
mod which {
    use std::path::PathBuf;

    pub fn which(name: &str) -> Option<PathBuf> {
        let path_var = std::env::var_os("PATH")?;
        std::env::split_paths(&path_var)
            .map(|dir| dir.join(name))
            .find(|full| full.is_file())
    }
}
