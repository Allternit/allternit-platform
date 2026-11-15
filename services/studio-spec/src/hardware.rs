//! Training hardware ids. Every computer maps to exactly one. The host owns
//! detection (it probes the OS, CPU and GPU vendor); [`classify`] is the shared
//! mapping from what it found to an id, so the host, the engines and the
//! tests agree on it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hardware {
    /// macOS on Apple Silicon: MLX or PyTorch with MPS.
    AppleSilicon,
    /// Linux with an NVIDIA GPU.
    CudaLinux,
    /// Windows with an NVIDIA GPU.
    CudaWindows,
    /// Linux with an AMD GPU (ROCm).
    RocmLinux,
    /// Linux with an Intel GPU (XPU).
    XpuLinux,
    /// Windows with an Intel GPU (XPU).
    XpuWindows,
    /// No GPU training can use. Also AMD on Windows and Intel Macs.
    Cpu,
}

impl Hardware {
    pub const ALL: [Hardware; 7] = [
        Hardware::AppleSilicon,
        Hardware::CudaLinux,
        Hardware::CudaWindows,
        Hardware::RocmLinux,
        Hardware::XpuLinux,
        Hardware::XpuWindows,
        Hardware::Cpu,
    ];

    /// The id used in engines.json and on the wire.
    pub fn id(self) -> &'static str {
        match self {
            Hardware::AppleSilicon => "apple-silicon",
            Hardware::CudaLinux => "cuda-linux",
            Hardware::CudaWindows => "cuda-windows",
            Hardware::RocmLinux => "rocm-linux",
            Hardware::XpuLinux => "xpu-linux",
            Hardware::XpuWindows => "xpu-windows",
            Hardware::Cpu => "cpu",
        }
    }

    pub fn from_id(id: &str) -> Option<Hardware> {
        Hardware::ALL.into_iter().find(|h| h.id() == id)
    }

    /// "a Mac with Apple Silicon", "Linux with an NVIDIA GPU", ...
    pub fn label(self) -> &'static str {
        match self {
            Hardware::AppleSilicon => "a Mac with Apple Silicon",
            Hardware::CudaLinux => "Linux with an NVIDIA GPU",
            Hardware::CudaWindows => "Windows with an NVIDIA GPU",
            Hardware::RocmLinux => "Linux with an AMD GPU",
            Hardware::XpuLinux => "Linux with an Intel GPU",
            Hardware::XpuWindows => "Windows with an Intel GPU",
            Hardware::Cpu => "the CPU alone",
        }
    }

    /// The torch device backend name for this hardware.
    pub fn torch_backend(self) -> &'static str {
        match self {
            Hardware::AppleSilicon => "torch-mps",
            Hardware::CudaLinux | Hardware::CudaWindows => "torch-cuda",
            Hardware::RocmLinux => "torch-rocm",
            Hardware::XpuLinux | Hardware::XpuWindows => "torch-xpu",
            Hardware::Cpu => "torch-cpu",
        }
    }
}

/// GPU makers the host can find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
}

/// The id a computer maps to, plus why when it falls back to `cpu` although
/// it has a GPU (AMD on Windows, an Intel Mac).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostHardware {
    pub hardware: Hardware,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Set when PyTorch cannot run on this computer at all (Intel Macs: the
    /// last torch build there is 2.2.2, and the engines need 2.6+). Every
    /// torch-based engine's `fit` is then `ok: false` with this reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torch_unavailable: Option<String>,
}

/// Why no torch engine runs on an Intel Mac.
pub const INTEL_MAC_NO_TORCH: &str =
    "Training needs PyTorch 2.6 or newer, which has no build for Intel Macs.";

impl HostHardware {
    pub fn plain(hardware: Hardware) -> HostHardware {
        HostHardware { hardware, reason: None, torch_unavailable: None }
    }
}

/// Map what the host found to a hardware id. `os` and `arch` use Rust's
/// `std::env::consts` names (`macos`, `linux`, `windows`; `aarch64`, `x86_64`).
/// `gpus` lists the vendors of GPUs with a working driver. NVIDIA wins over
/// AMD, AMD over Intel (an Intel iGPU often sits next to a discrete GPU).
pub fn classify(os: &str, arch: &str, gpus: &[GpuVendor]) -> HostHardware {
    let has = |v: GpuVendor| gpus.contains(&v);
    let cpu = |why: &str| HostHardware {
        hardware: Hardware::Cpu,
        reason: Some(why.to_string()),
        torch_unavailable: None,
    };
    match os {
        "macos" if arch == "aarch64" => HostHardware::plain(Hardware::AppleSilicon),
        "macos" => HostHardware {
            torch_unavailable: Some(INTEL_MAC_NO_TORCH.to_string()),
            ..cpu("Intel Macs have no GPU that PyTorch or MLX can train on, so training uses the CPU.")
        },
        "linux" if has(GpuVendor::Nvidia) => HostHardware::plain(Hardware::CudaLinux),
        "linux" if has(GpuVendor::Amd) => HostHardware::plain(Hardware::RocmLinux),
        "linux" if has(GpuVendor::Intel) => HostHardware::plain(Hardware::XpuLinux),
        "windows" if has(GpuVendor::Nvidia) => HostHardware::plain(Hardware::CudaWindows),
        "windows" if has(GpuVendor::Intel) => HostHardware::plain(Hardware::XpuWindows),
        "windows" if has(GpuVendor::Amd) => cpu("PyTorch cannot train on AMD GPUs on Windows, so training uses the CPU."),
        "linux" | "windows" => cpu("No NVIDIA, AMD or Intel GPU was found, so training uses the CPU."),
        _ => cpu("Training on this operating system uses the CPU."),
    }
}
