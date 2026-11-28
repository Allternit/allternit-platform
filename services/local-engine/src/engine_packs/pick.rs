//! Which engine pack this machine should run, from what `GET /hardware` found.
//!
//! Rules (plan "On-device models", W6):
//! - Apple silicon: llama.cpp Metal, plus the MLX sidecar.
//! - NVIDIA: CUDA when the driver answers (`nvidia-smi` listed the card),
//!   else Vulkan.
//! - AMD: ROCm on Linux when the ROCm runtime is installed, else Vulkan.
//! - Intel: Vulkan (SYCL later).
//! - Windows on Arm: CPU first, Vulkan as the second choice.
//! - Anything else: CPU.
//!
//! Vulkan is only offered when the Vulkan loader is present; the CPU pack is
//! always the last choice so a machine never ends up with nothing.

use crate::hardware::{EngineBackend, GpuVendor, HardwareGpu};

/// What the pick needs to know. Built from the hardware report plus a few
/// cheap file checks ([`PickInput::detect`]); tests build it by hand.
#[derive(Debug, Clone)]
pub struct PickInput {
    /// `macos`, `windows` or `linux` (Rust `std::env::consts::OS`).
    pub os: String,
    /// `aarch64` or `x86_64` (Rust `std::env::consts::ARCH`).
    pub arch: String,
    pub gpus: Vec<HardwareGpu>,
    pub apple_silicon: bool,
    /// The NVIDIA driver answered (nvidia-smi listed a card).
    pub nvidia_driver: bool,
    /// ROCm runtime installed (Linux): `/opt/rocm` or `/dev/kfd`.
    pub rocm_runtime: bool,
    /// Vulkan loader present (`vulkan-1.dll` / `libvulkan.so.1`).
    pub vulkan_loader: bool,
}

/// One pack the machine could use, best first.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PackChoice {
    pub pack_id: String,
    /// `llamacpp`, `mlx` or `colibri` (the `engine` field of pack.json).
    pub engine: &'static str,
    pub backend: EngineBackend,
}

/// Pack platform words, as `scripts/engine-packs/make-manifest.py` writes them.
fn os_word(os: &str) -> Option<&'static str> {
    match os {
        "macos" => Some("macos"),
        "windows" => Some("windows"),
        "linux" => Some("linux"),
        _ => None,
    }
}

fn arch_word(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" | "arm64" => Some("arm64"),
        "x86_64" | "x64" => Some("x64"),
        _ => None,
    }
}

pub fn backend_word(b: EngineBackend) -> &'static str {
    match b {
        EngineBackend::Metal => "metal",
        EngineBackend::Cuda => "cuda",
        EngineBackend::Rocm => "rocm",
        EngineBackend::Vulkan => "vulkan",
        EngineBackend::Sycl => "sycl",
        EngineBackend::Cpu => "cpu",
    }
}

pub fn parse_backend(word: &str) -> Option<EngineBackend> {
    Some(match word {
        "metal" => EngineBackend::Metal,
        "cuda" => EngineBackend::Cuda,
        "rocm" => EngineBackend::Rocm,
        "vulkan" => EngineBackend::Vulkan,
        "sycl" => EngineBackend::Sycl,
        "cpu" => EngineBackend::Cpu,
        _ => return None,
    })
}

/// `llamacpp-metal-macos-arm64` style id.
pub fn pack_id(engine: &str, backend: EngineBackend, os: &str, arch: &str) -> Option<String> {
    Some(format!("{engine}-{}-{}-{}", backend_word(backend), os_word(os)?, arch_word(arch)?))
}

/// Platforms the pack CI builds (`.github/workflows/engine-packs.yml`).
/// A pick outside this list would 404 on the feed, so it is not offered.
const BUILT: &[(&str, &str, &str, &str)] = &[
    ("llamacpp", "metal", "macos", "arm64"),
    ("mlx", "metal", "macos", "arm64"),
    ("llamacpp", "cuda", "windows", "x64"),
    ("llamacpp", "vulkan", "windows", "x64"),
    ("llamacpp", "cpu", "windows", "x64"),
    ("llamacpp", "cpu", "windows", "arm64"),
    ("llamacpp", "vulkan", "windows", "arm64"),
    ("llamacpp", "cuda", "linux", "x64"),
    ("llamacpp", "rocm", "linux", "x64"),
    ("llamacpp", "vulkan", "linux", "x64"),
    ("llamacpp", "cpu", "linux", "x64"),
    ("llamacpp", "cpu", "linux", "arm64"),
];

/// colibri packs (contract section 3.4): CPU builds from the v2.2.0 release.
/// Ids follow the contract, not the `engine-backend-os-arch` shape above.
/// There is no macOS x64 or Windows Arm archive, and no Metal pack in v1.
const COLIBRI_BUILT: &[(&str, &str, &str)] = &[
    ("linux", "x64", "colibri-linux-x86_64"),
    ("linux", "arm64", "colibri-linux-aarch64"),
    ("windows", "x64", "colibri-windows-x86_64"),
    ("macos", "arm64", "colibri-macos-arm64"),
];

fn built(engine: &str, backend: EngineBackend, os: &str, arch: &str) -> bool {
    let (Some(os), Some(arch)) = (os_word(os), arch_word(arch)) else { return false };
    BUILT.iter().any(|(e, b, o, a)| *e == engine && *b == backend_word(backend) && *o == os && *a == arch)
}

/// llama.cpp packs for this machine, best first. Empty when no pack is built
/// for this OS/arch (Intel Macs, for example): the PATH fallback applies.
pub fn llamacpp_choices(input: &PickInput) -> Vec<PackChoice> {
    use EngineBackend::*;
    let has = |v: GpuVendor| input.gpus.iter().any(|g| g.vendor == v);
    let arm = arch_word(&input.arch) == Some("arm64");
    let mut order: Vec<EngineBackend> = Vec::new();
    if input.apple_silicon {
        order.push(Metal);
    } else if input.os == "windows" && arm {
        order.push(Cpu);
        if input.vulkan_loader {
            order.push(Vulkan);
        }
    } else {
        if has(GpuVendor::Nvidia) && input.nvidia_driver {
            order.push(Cuda);
        }
        if has(GpuVendor::Amd) && input.os == "linux" && input.rocm_runtime {
            order.push(Rocm);
        }
        let gpu = has(GpuVendor::Nvidia) || has(GpuVendor::Amd) || has(GpuVendor::Intel);
        if gpu && input.vulkan_loader {
            order.push(Vulkan);
        }
        order.push(Cpu);
    }
    order
        .into_iter()
        .filter(|b| built("llamacpp", *b, &input.os, &input.arch))
        .filter_map(|b| {
            Some(PackChoice { pack_id: pack_id("llamacpp", b, &input.os, &input.arch)?, engine: "llamacpp", backend: b })
        })
        .collect()
}

/// The colibri CPU pack for this machine, or empty when none is built for this
/// OS/arch (Intel Macs, Windows on Arm). The CPU pack is the only one in v1,
/// so the list has at most one entry.
pub fn colibri_choices(input: &PickInput) -> Vec<PackChoice> {
    let (Some(os), Some(arch)) = (os_word(&input.os), arch_word(&input.arch)) else { return Vec::new() };
    COLIBRI_BUILT
        .iter()
        .filter(|(o, a, _)| *o == os && *a == arch)
        .map(|(_, _, id)| PackChoice { pack_id: (*id).to_string(), engine: "colibri", backend: EngineBackend::Cpu })
        .collect()
}

/// The MLX sidecar pack: Apple silicon only.
pub fn mlx_choice(input: &PickInput) -> Option<PackChoice> {
    if !input.apple_silicon || !built("mlx", EngineBackend::Metal, &input.os, &input.arch) {
        return None;
    }
    Some(PackChoice {
        pack_id: pack_id("mlx", EngineBackend::Metal, &input.os, &input.arch)?,
        engine: "mlx",
        backend: EngineBackend::Metal,
    })
}

impl PickInput {
    /// Build from probed GPUs plus file checks for the vendor runtimes.
    pub fn detect(gpus: &[HardwareGpu], apple_silicon: bool) -> Self {
        let nvidia_driver = gpus
            .iter()
            .any(|g| g.vendor == GpuVendor::Nvidia && matches!(g.source, crate::hardware::gpu_probe::GpuSource::NvidiaSmi(_)));
        PickInput {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            gpus: gpus.to_vec(),
            apple_silicon,
            nvidia_driver,
            rocm_runtime: rocm_runtime_present(),
            vulkan_loader: vulkan_loader_present(),
        }
    }
}

fn rocm_runtime_present() -> bool {
    cfg!(target_os = "linux")
        && (std::path::Path::new("/opt/rocm").is_dir() || std::path::Path::new("/dev/kfd").exists())
}

fn vulkan_loader_present() -> bool {
    if cfg!(target_os = "windows") {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        return std::path::Path::new(&root).join("System32").join("vulkan-1.dll").is_file();
    }
    if cfg!(target_os = "linux") {
        return [
            "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
            "/usr/lib/aarch64-linux-gnu/libvulkan.so.1",
            "/usr/lib64/libvulkan.so.1",
            "/usr/lib/libvulkan.so.1",
        ]
        .iter()
        .any(|p| std::path::Path::new(p).exists());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::gpu_probe::GpuSource;
    use EngineBackend::*;

    fn gpu(vendor: GpuVendor, discrete: bool) -> HardwareGpu {
        HardwareGpu { name: "x".into(), vendor, vram_total_bytes: Some(8 << 30), vram_free_bytes: None, discrete, source: GpuSource::Static }
    }

    fn input(os: &str, arch: &str, gpus: Vec<HardwareGpu>) -> PickInput {
        PickInput { os: os.into(), arch: arch.into(), gpus, apple_silicon: false, nvidia_driver: false, rocm_runtime: false, vulkan_loader: true }
    }

    fn backends(i: &PickInput) -> Vec<EngineBackend> {
        llamacpp_choices(i).into_iter().map(|c| c.backend).collect()
    }

    #[test]
    fn nvidia_needs_the_driver_for_cuda() {
        let mut i = input("windows", "x86_64", vec![gpu(GpuVendor::Nvidia, true)]);
        i.nvidia_driver = true;
        assert_eq!(backends(&i), vec![Cuda, Vulkan, Cpu]);
        assert_eq!(llamacpp_choices(&i)[0].pack_id, "llamacpp-cuda-windows-x64");
        i.nvidia_driver = false;
        assert_eq!(backends(&i), vec![Vulkan, Cpu]);
        i.vulkan_loader = false;
        assert_eq!(backends(&i), vec![Cpu]);
    }

    #[test]
    fn amd_takes_rocm_only_on_linux_with_the_runtime() {
        let mut i = input("linux", "x86_64", vec![gpu(GpuVendor::Amd, true)]);
        i.rocm_runtime = true;
        assert_eq!(backends(&i), vec![Rocm, Vulkan, Cpu]);
        i.rocm_runtime = false;
        assert_eq!(backends(&i), vec![Vulkan, Cpu]);
        let mut w = input("windows", "x86_64", vec![gpu(GpuVendor::Amd, true)]);
        w.rocm_runtime = true; // no ROCm pack on Windows
        assert_eq!(backends(&w), vec![Vulkan, Cpu]);
    }

    #[test]
    fn intel_takes_vulkan() {
        let i = input("linux", "x86_64", vec![gpu(GpuVendor::Intel, false)]);
        assert_eq!(backends(&i), vec![Vulkan, Cpu]);
        assert_eq!(llamacpp_choices(&i)[0].pack_id, "llamacpp-vulkan-linux-x64");
    }

    #[test]
    fn windows_on_arm_is_cpu_first() {
        let i = input("windows", "aarch64", vec![gpu(GpuVendor::Qualcomm, false)]);
        assert_eq!(backends(&i), vec![Cpu, Vulkan]);
        let mut n = i.clone();
        n.vulkan_loader = false;
        assert_eq!(backends(&n), vec![Cpu]);
        assert_eq!(llamacpp_choices(&n)[0].pack_id, "llamacpp-cpu-windows-arm64");
    }

    #[test]
    fn no_gpu_is_cpu_and_unbuilt_platforms_get_nothing() {
        let i = input("linux", "aarch64", vec![]);
        assert_eq!(backends(&i), vec![Cpu]);
        assert_eq!(llamacpp_choices(&i)[0].pack_id, "llamacpp-cpu-linux-arm64");
        // Intel Mac: no pack is built, so the PATH fallback is the only option.
        let mac = input("macos", "x86_64", vec![gpu(GpuVendor::Amd, true)]);
        assert!(llamacpp_choices(&mac).is_empty());
        assert!(mlx_choice(&mac).is_none());
    }

    #[test]
    fn apple_silicon_gets_metal_and_mlx() {
        let mut i = input("macos", "aarch64", vec![gpu(GpuVendor::Apple, false)]);
        i.apple_silicon = true;
        assert_eq!(backends(&i), vec![Metal]);
        assert_eq!(llamacpp_choices(&i)[0].pack_id, "llamacpp-metal-macos-arm64");
        assert_eq!(mlx_choice(&i).unwrap().pack_id, "mlx-metal-macos-arm64");
        // MLX never applies off Apple silicon.
        assert!(mlx_choice(&input("linux", "x86_64", vec![])).is_none());
    }

    #[test]
    fn colibri_has_one_cpu_pack_per_built_platform() {
        let id = |os: &str, arch: &str| colibri_choices(&input(os, arch, vec![])).into_iter().map(|c| c.pack_id).collect::<Vec<_>>();
        assert_eq!(id("linux", "x86_64"), vec!["colibri-linux-x86_64"]);
        assert_eq!(id("linux", "aarch64"), vec!["colibri-linux-aarch64"]);
        assert_eq!(id("windows", "x86_64"), vec!["colibri-windows-x86_64"]);
        let mut mac = input("macos", "aarch64", vec![gpu(GpuVendor::Apple, false)]);
        mac.apple_silicon = true;
        let c = colibri_choices(&mac);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].pack_id.as_str(), c[0].engine, c[0].backend), ("colibri-macos-arm64", "colibri", Cpu));
    }

    #[test]
    fn colibri_gets_nothing_where_no_archive_exists() {
        // No macOS x64 and no Windows Arm archive in the v2.2.0 release.
        assert!(colibri_choices(&input("macos", "x86_64", vec![])).is_empty());
        assert!(colibri_choices(&input("windows", "aarch64", vec![])).is_empty());
        assert!(colibri_choices(&input("freebsd", "x86_64", vec![])).is_empty());
        // A GPU never changes the pick: v1 packs are CPU only.
        let mut n = input("windows", "x86_64", vec![gpu(GpuVendor::Nvidia, true)]);
        n.nvidia_driver = true;
        assert_eq!(colibri_choices(&n).len(), 1);
    }
}
