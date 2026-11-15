//! Hardware profiling for the local engine.
//!
//! Detects CPU, RAM, GPU, OS, and backend capabilities; produces a stable
//! `hardware_id`; and persists the profile so the frontend can read it without
//! re-running detection on every request.

use serde::{Deserialize, Serialize};
use std::path::Path;
use sysinfo::System;

pub mod gpu_probe;

/// Detected backend capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub metal: bool,
    pub cuda: bool,
    pub cpu_fallback: bool,
}

impl Default for BackendCapabilities {
    fn default() -> Self {
        Self {
            metal: false,
            cuda: false,
            cpu_fallback: true,
        }
    }
}

/// Normalized hardware profile used for model assessment and recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub hardware_id: String,
    pub os: String,
    pub arch: String,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub cpu_threads: usize,
    pub ram_total_bytes: u64,
    pub ram_used_bytes: u64,
    pub gpu_name: Option<String>,
    pub gpu_memory_total_bytes: Option<u64>,
    pub gpu_memory_used_bytes: Option<u64>,
    pub apple_chip: Option<String>,
    pub unified_memory: bool,
    pub backends: BackendCapabilities,
}

impl HardwareProfile {
    /// Total memory budget for model loading.
    ///
    /// On Apple Silicon (unified memory) this is the system RAM pool.
    /// On CUDA it is the dedicated VRAM budget.
    /// Otherwise it is the total system RAM.
    pub fn memory_budget_bytes(&self) -> u64 {
        if self.unified_memory {
            return self.ram_total_bytes;
        }
        if self.backends.cuda {
            return self.gpu_memory_total_bytes.unwrap_or(self.ram_total_bytes);
        }
        self.ram_total_bytes
    }

    /// Free-ish memory currently available.
    pub fn available_memory_bytes(&self) -> u64 {
        let system_budget = self.ram_total_bytes.saturating_sub(self.ram_used_bytes);
        if self.unified_memory {
            return system_budget;
        }
        if let Some(vram) = self.gpu_memory_total_bytes {
            let vram_budget = vram.saturating_sub(self.gpu_memory_used_bytes.unwrap_or(0));
            return vram_budget.min(system_budget);
        }
        system_budget
    }
}

/// Detect the current hardware profile, optionally caching it to `data_dir`.
pub fn detect_and_persist(data_dir: impl AsRef<Path>) -> HardwareProfile {
    let profile = detect();
    let cache_path = data_dir.as_ref().join("hardware_profile.json");
    if let Ok(json) = serde_json::to_string_pretty(&profile) {
        let _ = std::fs::write(&cache_path, json);
    }
    profile
}

/// Read the cached hardware profile if it exists, otherwise detect and cache.
pub fn load_or_detect(data_dir: impl AsRef<Path>) -> HardwareProfile {
    let cache_path = data_dir.as_ref().join("hardware_profile.json");
    if let Ok(bytes) = std::fs::read(&cache_path) {
        if let Ok(profile) = serde_json::from_slice::<HardwareProfile>(&bytes) {
            return profile;
        }
    }
    detect_and_persist(data_dir)
}

/// Detect the current hardware profile.
pub fn detect() -> HardwareProfile {
    let sys = System::new_all();

    let os = System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string());
    let arch = std::env::consts::ARCH.to_string();
    let kernel = System::kernel_version();
    let hostname = System::host_name();

    let cpus = sys.cpus();
    let cpu_model = cpus
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());
    let cpu_cores = sys.physical_core_count().unwrap_or(cpus.len());
    let cpu_threads = cpus.len();

    let ram_total_bytes = sys.total_memory();
    let ram_used_bytes = sys.used_memory();

    let (gpu_name, gpu_memory_total_bytes, gpu_memory_used_bytes, apple_chip, unified_memory) =
        detect_gpu_info(&sys);

    let backends = BackendCapabilities {
        metal: cfg!(target_os = "macos") && apple_chip.is_some(),
        cuda: cfg!(any(target_os = "linux", target_os = "windows")) && has_nvidia_gpu(),
        ..Default::default()
    };

    let hardware_id = stable_hardware_id(
        &cpu_model,
        cpu_cores,
        ram_total_bytes,
        gpu_name.as_deref(),
        &os,
        &arch,
    );

    HardwareProfile {
        hardware_id,
        os,
        arch,
        kernel,
        hostname,
        cpu_model,
        cpu_cores,
        cpu_threads,
        ram_total_bytes,
        ram_used_bytes,
        gpu_name,
        gpu_memory_total_bytes,
        gpu_memory_used_bytes,
        apple_chip,
        unified_memory,
        backends,
    }
}

fn stable_hardware_id(
    cpu_model: &str,
    cpu_cores: usize,
    ram_total_bytes: u64,
    gpu_name: Option<&str>,
    os: &str,
    arch: &str,
) -> String {
    let gpu = gpu_name.unwrap_or("none");
    let input = format!(
        "{}|{}|{}|{}|{}|{}",
        cpu_model, cpu_cores, ram_total_bytes, gpu, os, arch
    );
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

#[cfg(target_os = "macos")]
fn detect_gpu_info(
    sys: &System,
) -> (
    Option<String>,
    Option<u64>,
    Option<u64>,
    Option<String>,
    bool,
) {
    let ram_total_bytes = sys.total_memory();
    let ram_used_bytes = sys.used_memory();

    if let Some(output) = gpu_probe::run_with_timeout(
        gpu_probe::quiet_command("system_profiler").args(["SPDisplaysDataType", "-json"]),
        gpu_probe::PROBE_TIMEOUT,
    ) {
        if output.success {
            let text = output.stdout;
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                let displays = json
                    .get("SPDisplaysDataType")
                    .and_then(|v| v.as_array())
                    .or_else(|| json.as_array());
                if let Some(arr) = displays {
                    if let Some(display) = arr.first() {
                        let name = display["sppci_model"]
                            .as_str()
                            .or_else(|| display["_name"].as_str())
                            .unwrap_or("Apple GPU");
                        let is_apple = name.to_lowercase().contains("apple");
                        let apple_chip = apple_chip_generation(name);
                        let total = is_apple.then_some(ram_total_bytes);
                        let used = is_apple.then_some(ram_used_bytes);
                        return (Some(name.to_string()), total, used, apple_chip, is_apple);
                    }
                }
            }
        }
    }

    (None, None, None, None, false)
}

#[cfg(target_os = "linux")]
fn detect_gpu_info(
    _sys: &System,
) -> (
    Option<String>,
    Option<u64>,
    Option<u64>,
    Option<String>,
    bool,
) {
    match gpu_probe::nvidia_smi().into_iter().next() {
        Some(gpu) => (
            Some(gpu.name),
            gpu.memory_total_mb.map(|mb| mb * 1024 * 1024),
            gpu.memory_used_mb.map(|mb| mb * 1024 * 1024),
            None,
            false,
        ),
        None => (None, None, None, None, false),
    }
}

/// Windows: nvidia-smi for NVIDIA cards (exact VRAM and usage), else the
/// first display adapter WMI reports (AMD/Intel; VRAM when WMI knows it).
#[cfg(target_os = "windows")]
fn detect_gpu_info(
    _sys: &System,
) -> (
    Option<String>,
    Option<u64>,
    Option<u64>,
    Option<String>,
    bool,
) {
    match gpu_probe::windows_gpus().into_iter().next() {
        Some(gpu) => (
            Some(gpu.name),
            gpu.memory_total_mb.map(|mb| mb * 1024 * 1024),
            gpu.memory_used_mb.map(|mb| mb * 1024 * 1024),
            None,
            false,
        ),
        None => (None, None, None, None, false),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn detect_gpu_info(
    _sys: &System,
) -> (
    Option<String>,
    Option<u64>,
    Option<u64>,
    Option<String>,
    bool,
) {
    (None, None, None, None, false)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// Chip generation ("M1" … "M5" and later) from a name like "Apple M3 Pro".
fn apple_chip_generation(name: &str) -> Option<String> {
    regex::Regex::new(r"(?i)\bM(\d{1,2})\b")
        .ok()?
        .captures(name)
        .map(|c| format!("M{}", &c[1]))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn has_nvidia_gpu() -> bool {
    !gpu_probe::nvidia_smi().is_empty()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn has_nvidia_gpu() -> bool {
    false
}

// ─── GET /hardware report ───────────────────────────────────────────────────
//
// The on-device fit contract (plan "On-device models", section 3; fit spec
// `spec/on-device/fit.md`). Clients feed `budget` straight into `fit()`.

pub use gpu_probe::{GpuVendor, HardwareGpu};

/// Which memory pool a model loads into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetKind {
    Discrete,
    Unified,
    Cpu,
}

/// Memory budget for model loading, as the fit spec defines it.
/// discrete: `fast` = VRAM of the largest discrete GPU, `spill` = system RAM.
/// unified: `fast` = 0.65 x RAM on Apple silicon, 0.60 x RAM otherwise.
/// cpu: `fast` = 0.6 x RAM. `spill` is 0 for unified and cpu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Budget {
    pub kind: BudgetKind,
    pub fast_bytes: u64,
    pub spill_bytes: u64,
}

/// llama.cpp / MLX compute backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineBackend {
    Metal,
    Cuda,
    Rocm,
    Vulkan,
    Sycl,
    Cpu,
}

/// ready: runs on the best backend for this machine. installed: present but
/// it could not be asked which devices it sees. fallback: present, but runs
/// on the CPU although this machine has a usable GPU. missing: not installed
/// (`backend` is the one to install). off: does not apply on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineState {
    Ready,
    Installed,
    Fallback,
    Missing,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineInfo {
    /// `llama.cpp` or `mlx`.
    pub id: &'static str,
    pub backend: EngineBackend,
    pub version: Option<String>,
    /// Engine pack that provides it; null for a binary found on PATH.
    pub pack_id: Option<String>,
    pub state: EngineState,
}

/// `GET /hardware` body. Field order follows the contract.
#[derive(Debug, Clone, Serialize)]
pub struct HardwareReport {
    pub hardware_id: String,
    /// `macos`, `linux` or `windows`.
    pub os: String,
    /// `aarch64` or `x86_64`.
    pub arch: String,
    pub cpu_model: String,
    pub cpu_threads: usize,
    pub ram_total_bytes: u64,
    pub ram_available_bytes: u64,
    pub gpus: Vec<HardwareGpu>,
    pub unified_memory: bool,
    /// Apple silicon chip ("M1 Pro"), null elsewhere.
    pub apple_chip: Option<String>,
    /// Free space on the volume that holds `models_dir`.
    pub disk_free_bytes: u64,
    pub models_dir: String,
    pub budget: Budget,
    pub engines: Vec<EngineInfo>,
    /// Loaded models with live numbers (filled per request, not cached).
    pub running: Vec<crate::runtime::RunningInfo>,
}

fn scale(bytes: u64, percent: u64) -> u64 {
    (bytes as u128 * percent as u128 / 100) as u64
}

/// Budget from the probed GPUs (fit spec, "How fast_bytes is built").
/// A discrete GPU counts only when its VRAM is known; otherwise any
/// integrated GPU (or Apple silicon) makes it unified, else cpu.
pub fn derive_budget(gpus: &[HardwareGpu], ram_total_bytes: u64, apple_silicon: bool) -> Budget {
    let largest_vram = gpus
        .iter()
        .filter(|g| g.discrete)
        .filter_map(|g| g.vram_total_bytes)
        .filter(|v| *v > 0)
        .max();
    if let Some(vram) = largest_vram {
        return Budget { kind: BudgetKind::Discrete, fast_bytes: vram, spill_bytes: ram_total_bytes };
    }
    if apple_silicon {
        return Budget { kind: BudgetKind::Unified, fast_bytes: scale(ram_total_bytes, 65), spill_bytes: 0 };
    }
    if gpus.iter().any(|g| !g.discrete) {
        return Budget { kind: BudgetKind::Unified, fast_bytes: scale(ram_total_bytes, 60), spill_bytes: 0 };
    }
    Budget { kind: BudgetKind::Cpu, fast_bytes: scale(ram_total_bytes, 60), spill_bytes: 0 }
}

/// The llama.cpp backend this machine should run: what a pack should
/// provide, and what a found binary is measured against.
pub fn preferred_llamacpp_backend(gpus: &[HardwareGpu], apple_silicon: bool) -> EngineBackend {
    if apple_silicon {
        return EngineBackend::Metal;
    }
    let has = |v: GpuVendor| gpus.iter().any(|g| g.vendor == v && g.discrete);
    if has(GpuVendor::Nvidia) {
        EngineBackend::Cuda
    } else if has(GpuVendor::Amd) {
        if cfg!(target_os = "linux") { EngineBackend::Rocm } else { EngineBackend::Vulkan }
    } else if has(GpuVendor::Intel) {
        EngineBackend::Vulkan
    } else {
        EngineBackend::Cpu
    }
}

/// `llama-server --version` → `b10330` from "version: 10330 (687e77892)".
pub fn parse_llama_version(text: &str) -> Option<String> {
    let rest = text.lines().find_map(|l| l.trim().strip_prefix("version:"))?.trim();
    let build = rest.split_whitespace().next()?;
    Some(if build.chars().all(|c| c.is_ascii_digit()) { format!("b{build}") } else { build.to_string() })
}

/// GPU backends listed by `llama-server --list-devices` ("MTL0: Apple M1 Pro",
/// "CUDA0: …", "ROCm0: …", "Vulkan0: …", "SYCL0: …"; BLAS/CPU/RPC ignored).
/// `None` when the output has no device list (older builds).
pub fn parse_llama_devices(text: &str) -> Option<Vec<EngineBackend>> {
    let mut lines = text.lines().skip_while(|l| !l.trim_start().starts_with("Available devices"));
    lines.next()?;
    Some(
        lines
            .filter_map(|l| {
                let (dev, _) = l.trim().split_once(':')?;
                let kind = dev.trim_end_matches(|c: char| c.is_ascii_digit()).to_ascii_lowercase();
                match kind.as_str() {
                    "mtl" | "metal" => Some(EngineBackend::Metal),
                    "cuda" => Some(EngineBackend::Cuda),
                    "rocm" | "hip" => Some(EngineBackend::Rocm),
                    "vulkan" => Some(EngineBackend::Vulkan),
                    "sycl" => Some(EngineBackend::Sycl),
                    _ => None,
                }
            })
            .collect(),
    )
}

/// State of a found llama.cpp binary given what it reported.
pub fn llamacpp_state(
    devices: Option<&[EngineBackend]>,
    preferred: EngineBackend,
) -> (EngineState, EngineBackend) {
    const ORDER: [EngineBackend; 5] = [
        EngineBackend::Cuda,
        EngineBackend::Rocm,
        EngineBackend::Metal,
        EngineBackend::Vulkan,
        EngineBackend::Sycl,
    ];
    let Some(devices) = devices else {
        return (EngineState::Installed, preferred);
    };
    if devices.contains(&preferred) {
        return (EngineState::Ready, preferred);
    }
    match ORDER.iter().find(|b| devices.contains(b)) {
        Some(b) => (EngineState::Ready, *b),
        None if preferred == EngineBackend::Cpu => (EngineState::Ready, EngineBackend::Cpu),
        None => (EngineState::Fallback, EngineBackend::Cpu),
    }
}

/// Find a program on PATH (`.exe` on Windows).
pub fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(&file))
        .find(|full| full.is_file())
}

/// Engine state from an installed, signed pack (files are re-hashed before
/// every spawn, not here). A CPU pack on a machine that has a usable GPU is
/// `fallback`.
fn pack_engine(id: &'static str, pack: &crate::engine_packs::InstalledPack, preferred: EngineBackend) -> EngineInfo {
    let backend = crate::engine_packs::pick::parse_backend(&pack.backend).unwrap_or(EngineBackend::Cpu);
    let state = if backend == EngineBackend::Cpu && preferred != EngineBackend::Cpu {
        EngineState::Fallback
    } else {
        EngineState::Ready
    };
    EngineInfo { id, backend, version: Some(pack.version.clone()), pack_id: Some(pack.pack_id.clone()), state }
}

fn detect_llamacpp(gpus: &[HardwareGpu], apple_silicon: bool, packs: &[crate::engine_packs::InstalledPack]) -> EngineInfo {
    let preferred = preferred_llamacpp_backend(gpus, apple_silicon);
    // An installed pack wins (the runtime manager launches it first); the
    // best backend among installed llama.cpp packs is the one it will use.
    let best = packs
        .iter()
        .filter(|p| p.engine == "llamacpp")
        .min_by_key(|p| match crate::engine_packs::pick::parse_backend(&p.backend) {
            Some(b) if b == preferred => 0,
            Some(EngineBackend::Cpu) | None => 2,
            Some(_) => 1,
        });
    if let Some(pack) = best {
        return pack_engine("llama.cpp", pack, preferred);
    }
    // No pack: `llama-server` on PATH is the development fallback.
    let Some(bin) = find_on_path("llama-server") else {
        return EngineInfo { id: "llama.cpp", backend: preferred, version: None, pack_id: None, state: EngineState::Missing };
    };
    // Run one after the other: both init the GPU backend, and two Metal inits
    // at once contend (measured 13 s vs 0.3 s each on an M1 Pro).
    let ask = |arg: &str| {
        gpu_probe::run_with_timeout(gpu_probe::quiet_command(&bin).arg(arg), gpu_probe::PROBE_TIMEOUT)
            .map(|o| format!("{}\n{}", o.stdout, o.stderr))
    };
    let devices = ask("--list-devices").and_then(|t| parse_llama_devices(&t));
    let version = ask("--version").and_then(|t| parse_llama_version(&t));
    let (state, backend) = llamacpp_state(devices.as_deref(), preferred);
    // A PATH binary is never `ready`: it is unsigned and unmanaged.
    let state = if state == EngineState::Installed { state } else { EngineState::Fallback };
    EngineInfo { id: "llama.cpp", backend, version, pack_id: None, state }
}

fn detect_mlx(apple_silicon: bool, packs: &[crate::engine_packs::InstalledPack]) -> EngineInfo {
    if !apple_silicon {
        return EngineInfo { id: "mlx", backend: EngineBackend::Metal, version: None, pack_id: None, state: EngineState::Off };
    }
    if let Some(pack) = packs.iter().find(|p| p.engine == "mlx") {
        return pack_engine("mlx", pack, EngineBackend::Metal);
    }
    let state = if find_on_path(crate::runtime::backends::mlx::BINARY).is_some() {
        EngineState::Fallback
    } else {
        EngineState::Missing
    };
    EngineInfo { id: "mlx", backend: EngineBackend::Metal, version: None, pack_id: None, state }
}

fn disk_free_bytes(path: &Path) -> u64 {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    disks
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
        .unwrap_or(0)
}

/// Available RAM from sysinfo's counters.
///
/// macOS (`macos = true`): sysinfo 0.30 computes available as free +
/// inactive + purgeable minus the pages the compressor occupies. Under heavy
/// compression the compressor holds more pages than that sum, so it
/// saturates to 0 (seen on a 32 GB M1 Pro: 644,894 compressor pages vs
/// 481,096 free + inactive + purgeable). Use total - used instead, where
/// used = active + wired + compressor + speculative: the Activity Monitor
/// view. Linux (MemAvailable) and Windows (ullAvailPhys) keep sysinfo's value.
pub fn available_from(total: u64, used: u64, reported_available: u64, macos: bool) -> u64 {
    if macos {
        total.saturating_sub(used)
    } else {
        reported_available
    }
}

/// (total, available) system RAM in bytes.
fn memory() -> (u64, u64) {
    let mut sys = System::new();
    sys.refresh_memory();
    let total = sys.total_memory();
    let available = available_from(total, sys.used_memory(), sys.available_memory(), cfg!(target_os = "macos"));
    (total, available)
}

/// Full probe: GPUs, engines, budget. Blocking (runs commands with
/// timeouts); call it from `spawn_blocking`. Never fails.
pub fn probe_report(
    profile: &HardwareProfile,
    models_dir: &Path,
    packs: &[crate::engine_packs::InstalledPack],
) -> HardwareReport {
    let (ram_total, ram_available) = memory();
    let gpus = gpu_probe::probe_gpus(ram_total, ram_available);
    let apple_gpu = gpus.iter().find(|g| g.source == gpu_probe::GpuSource::AppleUnified);
    let apple_silicon = apple_gpu.is_some();
    let apple_chip = apple_gpu.map(|g| g.name.trim_start_matches("Apple ").trim().to_string());
    let budget = derive_budget(&gpus, ram_total, apple_silicon);
    let engines = vec![detect_llamacpp(&gpus, apple_silicon, packs), detect_mlx(apple_silicon, packs)];
    HardwareReport {
        hardware_id: profile.hardware_id.clone(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_model: profile.cpu_model.clone(),
        cpu_threads: profile.cpu_threads,
        ram_total_bytes: ram_total,
        ram_available_bytes: ram_available,
        unified_memory: budget.kind == BudgetKind::Unified,
        gpus,
        apple_chip,
        disk_free_bytes: disk_free_bytes(models_dir),
        models_dir: models_dir.to_string_lossy().into_owned(),
        budget,
        engines,
        running: Vec::new(),
    }
}

/// Refresh only the fields that move: free RAM, free VRAM, free disk.
pub fn refresh_live(report: &mut HardwareReport, models_dir: &Path) {
    let (_, ram_available) = memory();
    apply_live(report, ram_available, disk_free_bytes(models_dir));
    gpu_probe::refresh_vram_free(&mut report.gpus, ram_available);
}

/// Write fresh free-memory readings into a cached report. The Apple unified
/// GPU's free memory is the free RAM; other GPUs are re-read by
/// `refresh_vram_free`.
fn apply_live(report: &mut HardwareReport, ram_available: u64, disk_free: u64) {
    report.ram_available_bytes = ram_available;
    report.disk_free_bytes = disk_free;
}

#[cfg(test)]
mod tests {
    use super::*;


    fn gpu(vendor: GpuVendor, vram: Option<u64>, discrete: bool) -> HardwareGpu {
        HardwareGpu { name: "x".into(), vendor, vram_total_bytes: vram, vram_free_bytes: None, discrete, source: Default::default() }
    }

    const GB: u64 = 1_000_000_000;

    #[test]
    fn budget_follows_the_fit_spec() {
        let ram = 32 * GB;
        // Largest discrete VRAM wins; RAM is the spill pool; iGPU ignored.
        let b = derive_budget(&[gpu(GpuVendor::Intel, None, false), gpu(GpuVendor::Nvidia, Some(8 * GB), true), gpu(GpuVendor::Nvidia, Some(12 * GB), true)], ram, false);
        assert_eq!(b, Budget { kind: BudgetKind::Discrete, fast_bytes: 12 * GB, spill_bytes: ram });
        // Apple silicon: 0.65 x RAM (48 GB Mac -> 31.2 GB).
        let b = derive_budget(&[gpu(GpuVendor::Apple, Some(48 * GB), false)], 48 * GB, true);
        assert_eq!(b, Budget { kind: BudgetKind::Unified, fast_bytes: 31_200_000_000, spill_bytes: 0 });
        // Other unified (Snapdragon / iGPU only): 0.60 x RAM.
        let b = derive_budget(&[gpu(GpuVendor::Qualcomm, None, false)], ram, false);
        assert_eq!(b, Budget { kind: BudgetKind::Unified, fast_bytes: 19_200_000_000, spill_bytes: 0 });
        // No GPU: cpu, 0.6 x RAM.
        let b = derive_budget(&[], 16 * GB, false);
        assert_eq!(b, Budget { kind: BudgetKind::Cpu, fast_bytes: 9_600_000_000, spill_bytes: 0 });
        // A discrete GPU with unknown VRAM can't set a budget: falls through.
        let b = derive_budget(&[gpu(GpuVendor::Amd, None, true)], 16 * GB, false);
        assert_eq!(b.kind, BudgetKind::Cpu);
        // No overflow on huge RAM.
        assert_eq!(derive_budget(&[], u64::MAX, false).fast_bytes, (u64::MAX as u128 * 60 / 100) as u64);
    }

    #[test]
    fn parses_llama_server_version_and_devices() {
        assert_eq!(parse_llama_version("version: 10330 (687e77892)\nbuilt with AppleClang 16 for Darwin arm64").as_deref(), Some("b10330"));
        assert_eq!(parse_llama_version("usage: llama-server ..."), None);

        let mac = "ggml_metal_init: ...\nAvailable devices:\n  BLAS: Accelerate (0 MiB, 0 MiB free)\n  MTL0: Apple M1 Pro (25559 MiB, 25558 MiB free)\n";
        assert_eq!(parse_llama_devices(mac), Some(vec![EngineBackend::Metal]));
        let cuda = "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24080 MiB, 23000 MiB free)\n  Vulkan0: NVIDIA GeForce RTX 4090 (24080 MiB)\n";
        assert_eq!(parse_llama_devices(cuda), Some(vec![EngineBackend::Cuda, EngineBackend::Vulkan]));
        assert_eq!(parse_llama_devices("Available devices:\n"), Some(vec![]));
        assert_eq!(parse_llama_devices("error: unknown argument: --list-devices"), None);
    }

    #[test]
    fn llamacpp_state_compares_against_the_machine() {
        use EngineBackend::*;
        assert_eq!(llamacpp_state(Some(&[Metal]), Metal), (EngineState::Ready, Metal));
        // CPU-only build on an NVIDIA box: runs, but slowly.
        assert_eq!(llamacpp_state(Some(&[]), Cuda), (EngineState::Fallback, Cpu));
        // CPU-only build on a CPU-only box is the right build.
        assert_eq!(llamacpp_state(Some(&[]), Cpu), (EngineState::Ready, Cpu));
        // Vulkan build on an NVIDIA box still uses the GPU.
        assert_eq!(llamacpp_state(Some(&[Vulkan]), Cuda), (EngineState::Ready, Vulkan));
        assert_eq!(llamacpp_state(None, Cuda), (EngineState::Installed, Cuda));
        assert_eq!(preferred_llamacpp_backend(&[gpu(GpuVendor::Intel, None, false), gpu(GpuVendor::Nvidia, Some(GB), true)], false), Cuda);
        assert_eq!(preferred_llamacpp_backend(&[gpu(GpuVendor::Qualcomm, None, false)], false), Cpu);
    }

    fn apple_report(dir: &Path) -> HardwareReport {
        let mut gpus = vec![gpu(GpuVendor::Apple, Some(32 * GB), false), gpu(GpuVendor::Intel, None, false)];
        gpus[0].source = gpu_probe::GpuSource::AppleUnified;
        HardwareReport {
            hardware_id: "x".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            cpu_model: "x".into(),
            cpu_threads: 1,
            ram_total_bytes: 32 * GB,
            ram_available_bytes: 0,
            gpus,
            unified_memory: true,
            apple_chip: None,
            disk_free_bytes: 0,
            models_dir: dir.to_string_lossy().into_owned(),
            budget: derive_budget(&[], 32 * GB, true),
            engines: vec![],
        }
    }

    #[test]
    fn compressed_mac_memory_is_not_zero() {
        // Regression (W3, 2026-11-15): vm_stat on a 32 GB M1 Pro under
        // compression. sysinfo's available = free + inactive + purgeable -
        // compressor = 3,906 + 471,711 + 5,479 - 644,894 pages, saturating to
        // 0, so /hardware showed 0 free RAM and 0 free VRAM.
        const PAGE: u64 = 16_384;
        let total = 2_097_152 * PAGE;
        let used = (474_974 + 447_351 + 644_894 + 2_128) * PAGE; // active + wired + compressor + speculative
        let sysinfo_available = 0;
        let available = available_from(total, used, sysinfo_available, true);
        assert_eq!(available, 527_805 * PAGE); // ~8.6 GB, close to Activity Monitor
        // Linux / Windows keep the OS figure.
        assert_eq!(available_from(total, used, 5 * GB, false), 5 * GB);

        // The live refresh carries it into both fields the fit UI reads.
        let dir = tempfile::tempdir().unwrap();
        let mut report = apple_report(dir.path());
        apply_live(&mut report, available, 7);
        gpu_probe::refresh_vram_free(&mut report.gpus, available);
        assert_eq!(report.ram_available_bytes, available);
        assert_eq!(report.gpus[0].vram_free_bytes, Some(available), "unified GPU tracks free RAM");
        assert_eq!(report.gpus[1].vram_free_bytes, None, "static GPUs are left alone");
        assert_eq!(report.disk_free_bytes, 7);
    }

    #[test]
    fn live_refresh_on_this_machine_is_plausible() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = apple_report(dir.path());
        for _ in 0..3 {
            refresh_live(&mut report, dir.path());
            let (total, _) = memory();
            assert!(report.ram_available_bytes > 0, "available RAM read 0");
            assert!(report.ram_available_bytes <= total);
            assert_eq!(report.gpus[0].vram_free_bytes, Some(report.ram_available_bytes));
            assert!(report.disk_free_bytes > 0);
        }
    }

    #[test]
    fn hardware_id_is_stable() {
        let a = stable_hardware_id(
            "Apple M3",
            12,
            36_000_000_000,
            Some("Apple M3"),
            "macos",
            "aarch64",
        );
        let b = stable_hardware_id(
            "Apple M3",
            12,
            36_000_000_000,
            Some("Apple M3"),
            "macos",
            "aarch64",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn apple_chip_parsing() {
        assert_eq!(apple_chip_generation("Apple M5"), Some("M5".to_string()));
        assert_eq!(apple_chip_generation("Apple M10 Ultra"), Some("M10".to_string()));
        assert_eq!(
            apple_chip_generation("Apple M3 Pro"),
            Some("M3".to_string())
        );
        assert_eq!(
            apple_chip_generation("Apple M4 Max"),
            Some("M4".to_string())
        );
        assert_eq!(apple_chip_generation("AMD Radeon"), None);
    }
}
