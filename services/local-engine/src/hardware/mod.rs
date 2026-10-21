//! Hardware profiling for the local engine.
//!
//! Detects CPU, RAM, GPU, OS, and backend capabilities; produces a stable
//! `hardware_id`; and persists the profile so the frontend can read it without
//! re-running detection on every request.

use serde::{Deserialize, Serialize};
use std::path::Path;
use sysinfo::System;

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

    let mut backends = BackendCapabilities::default();
    backends.metal = cfg!(target_os = "macos") && apple_chip.is_some();
    backends.cuda = cfg!(target_os = "linux") && has_nvidia_gpu();

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
    use std::process::Command;

    let ram_total_bytes = sys.total_memory();
    let ram_used_bytes = sys.used_memory();

    if let Ok(output) = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
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
    use std::process::Command;

    if let Ok(output) = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,memory.used",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let parts: Vec<&str> = text
                .lines()
                .next()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .collect();
            if !parts.is_empty() {
                let name = parts[0].to_string();
                let total_mb: Option<u64> = parts.get(1).and_then(|s| s.parse().ok());
                let used_mb: Option<u64> = parts.get(2).and_then(|s| s.parse().ok());
                return (
                    Some(name.clone()),
                    total_mb.map(|mb| mb * 1024 * 1024),
                    used_mb.map(|mb| mb * 1024 * 1024),
                    None,
                    false,
                );
            }
        }
    }

    (None, None, None, None, false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
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

fn apple_chip_generation(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    if lower.contains("m4") {
        Some("M4".to_string())
    } else if lower.contains("m3") {
        Some("M3".to_string())
    } else if lower.contains("m2") {
        Some("M2".to_string())
    } else if lower.contains("m1") {
        Some("M1".to_string())
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn has_nvidia_gpu() -> bool {
    std::process::Command::new("nvidia-smi")
        .arg("--query-gpu=name")
        .arg("--format=csv,noheader")
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn has_nvidia_gpu() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

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
