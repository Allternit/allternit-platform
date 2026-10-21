//! Service status and diagnostics routes.

use crate::AppState;
use axum::{extract::State, response::Json, routing::get, Router};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use sysinfo::{Disks, System};

/// GPU information when available.
#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub name: String,
    pub memory_total_mb: Option<u64>,
    pub memory_used_mb: Option<u64>,
}

/// CPU information.
#[derive(Debug, Clone, Serialize)]
pub struct CpuInfo {
    pub model: String,
    pub cores: usize,
    pub threads: usize,
}

/// RAM information.
#[derive(Debug, Clone, Serialize)]
pub struct RamInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub total_mb: u64,
    pub used_mb: u64,
}

/// Platform / OS information.
#[derive(Debug, Clone, Serialize)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
}

/// Disk usage for the models directory.
#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub path: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
}

/// Overall service status response.
#[derive(Debug, Clone, Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub active_runtimes: usize,
    pub cached_models: usize,
    pub platform: PlatformInfo,
    pub cpu: CpuInfo,
    pub cpu_usage_percent: f32,
    pub ram: RamInfo,
    pub disk: DiskInfo,
    pub gpu: Option<Vec<GpuInfo>>,
}

/// Create the status router.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/status", get(get_status))
        .with_state(state)
}

async fn get_status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let runtimes = state.manager.list_runtimes().await;
    let cached_models = state.store.list().await.len();
    let disk = disk_info(&state.models_dir).unwrap_or_else(|_| DiskInfo {
        path: state.models_dir.to_string_lossy().into_owned(),
        total_bytes: 0,
        free_bytes: 0,
        used_bytes: 0,
    });

    let sys = System::new_all();
    let platform = platform_info(&sys);
    let cpu = cpu_info(&sys);
    // Live CPU usage and memory pressure come from the background sampler —
    // a per-request System can only report totals, not usage deltas.
    let sample = state.sampler.latest();
    let ram = ram_info(&sys, &sample);
    let gpu = detect_gpu(&ram);

    Json(StatusResponse {
        status: "healthy".into(),
        active_runtimes: runtimes.len(),
        cached_models,
        platform,
        cpu,
        cpu_usage_percent: sample.global_cpu_usage,
        ram,
        disk,
        gpu,
    })
}

fn platform_info(_sys: &System) -> PlatformInfo {
    PlatformInfo {
        os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        arch: std::env::consts::ARCH.to_string(),
        kernel: System::kernel_version(),
        hostname: System::host_name(),
    }
}

fn cpu_info(sys: &System) -> CpuInfo {
    let cpus = sys.cpus();
    let model = cpus
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());

    CpuInfo {
        model,
        cores: sys.physical_core_count().unwrap_or(cpus.len()),
        threads: cpus.len(),
    }
}

fn ram_info(sys: &System, sample: &crate::sampler::SystemSample) -> RamInfo {
    // Live used/total memory comes from the background sampler; the
    // per-request system only backs the totals if the sampler has never
    // ticked (it is seeded at startup, so this is just a safety net).
    let total_bytes = if sample.memory_total_bytes > 0 {
        sample.memory_total_bytes
    } else {
        sys.total_memory()
    };
    let used_bytes = sample.memory_used_bytes;

    RamInfo {
        total_bytes,
        used_bytes,
        total_mb: total_bytes / (1024 * 1024),
        used_mb: used_bytes / (1024 * 1024),
    }
}

fn disk_info(path: &Path) -> Result<DiskInfo, std::io::Error> {
    let disks = Disks::new_with_refreshed_list();

    // Find the disk that contains the models directory.
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let disk = disks
        .iter()
        .find(|d| target.starts_with(d.mount_point()))
        .or_else(|| disks.iter().next());

    let disk =
        disk.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no disk found"))?;

    let total = disk.total_space();
    let free = disk.available_space();
    Ok(DiskInfo {
        path: disk.mount_point().to_string_lossy().into_owned(),
        total_bytes: total,
        free_bytes: free,
        used_bytes: total.saturating_sub(free),
    })
}

/// Detect GPUs. On Apple Silicon the integrated GPU shares the system RAM, so
/// we surface the total/used system memory as the GPU memory.
fn detect_gpu(ram: &RamInfo) -> Option<Vec<GpuInfo>> {
    #[cfg(target_os = "macos")]
    {
        detect_macos_gpu(ram).ok()
    }
    #[cfg(target_os = "linux")]
    {
        detect_linux_gpu().ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn detect_macos_gpu(ram: &RamInfo) -> Result<Vec<GpuInfo>, Box<dyn std::error::Error>> {
    // Prefer system_profiler because it is easier to parse.
    let output = std::process::Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            let mut gpus = Vec::new();
            let displays = json
                .get("SPDisplaysDataType")
                .and_then(|v| v.as_array())
                .or_else(|| json.as_array());
            if let Some(arr) = displays {
                for display in arr {
                    let name = display["sppci_model"]
                        .as_str()
                        .or_else(|| display["_name"].as_str())
                        .unwrap_or("Apple GPU");

                    // Apple Silicon integrated GPUs share system memory.
                    let is_apple = name.to_lowercase().contains("apple");
                    gpus.push(GpuInfo {
                        name: name.to_string(),
                        memory_total_mb: is_apple.then_some(ram.total_mb),
                        memory_used_mb: is_apple.then_some(ram.used_mb),
                    });
                }
            }
            if !gpus.is_empty() {
                return Ok(gpus);
            }
        }
    }

    // Fallback to ioreg.
    let output = std::process::Command::new("ioreg")
        .args(["-c", "IOPCIDevice", "-d", "2", "-l"])
        .output()?;

    if !output.status.success() {
        return Err("ioreg failed".into());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();
    for line in text.lines() {
        if let Some(pos) = line.find("\"model\"") {
            let rest = &line[pos..];
            if let Some(start) = rest.find('"') {
                let rest = &rest[start + 1..];
                if let Some(end) = rest.find('"') {
                    let name = &rest[..end];
                    let is_apple = name.to_lowercase().contains("apple");
                    gpus.push(GpuInfo {
                        name: name.to_string(),
                        memory_total_mb: is_apple.then_some(ram.total_mb),
                        memory_used_mb: is_apple.then_some(ram.used_mb),
                    });
                }
            }
        }
    }

    if gpus.is_empty() {
        Err("no GPU detected".into())
    } else {
        Ok(gpus)
    }
}

#[cfg(target_os = "linux")]
fn detect_linux_gpu() -> Result<Vec<GpuInfo>, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,memory.used",
            "--format=csv,noheader,nounits",
        ])
        .output()?;

    if !output.status.success() {
        return Err("nvidia-smi failed".into());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut gpus = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() >= 1 {
            gpus.push(GpuInfo {
                name: parts[0].to_string(),
                memory_total_mb: parts.get(1).and_then(|s| s.parse().ok()),
                memory_used_mb: parts.get(2).and_then(|s| s.parse().ok()),
            });
        }
    }

    if gpus.is_empty() {
        Err("no NVIDIA GPU detected".into())
    } else {
        Ok(gpus)
    }
}
