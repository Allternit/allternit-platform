//! Training hardware detection (Decision 7: the host owns it, not
//! local-engine's probe). The host probes the OS, CPU and GPUs once, then
//! [`allternit_studio_spec::classify`] maps what it found to one of the seven
//! hardware ids.
//!
//! What counts as a working GPU, per vendor:
//!
//! - NVIDIA: `nvidia-smi -L` lists a GPU, or (Linux) `/proc/driver/nvidia/version`
//!   exists (driver loaded).
//! - AMD: Linux only for training: `rocminfo` lists a `gfx` agent, or
//!   `rocm-smi` answers, or `/dev/kfd` exists. On Windows a Radeon in the video
//!   controller list is found so the `cpu` reason can say why.
//! - Intel: `sycl-ls` or `xpu-smi` lists a GPU; on Linux also a DRM card with
//!   vendor `0x8086` and a `/dev/dri/renderD*` node; on Windows an Intel Arc in
//!   the video controller list.
//!
//! Probes are cheap (a few short commands with a timeout) and cached for the
//! life of the process. A missing tool is just "not found", never an error.
//! [`device_unavailable`] re-checks at job start, so a GPU whose driver went
//! away gives 503 `hardware_unavailable` instead of a failed run.

use allternit_studio_spec::{classify, GpuVendor, Hardware, HostHardware};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// What the probes saw. Each field is filled only on the OS it applies to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeOutput {
    /// stdout of `nvidia-smi -L` when it exited 0.
    pub nvidia_smi: Option<String>,
    /// `/proc/driver/nvidia/version` exists.
    pub nvidia_proc: bool,
    /// stdout of `rocminfo` when it exited 0.
    pub rocminfo: Option<String>,
    /// `rocm-smi` exited 0.
    pub rocm_smi: bool,
    /// `/dev/kfd` exists (the ROCm kernel driver).
    pub kfd: bool,
    /// stdout of `sycl-ls` when it exited 0.
    pub sycl_ls: Option<String>,
    /// stdout of `xpu-smi discovery` when it exited 0.
    pub xpu_smi: Option<String>,
    /// Linux: PCI vendor ids (`0x8086`, ...) of DRM cards, read only when a
    /// `/dev/dri/renderD*` node exists.
    pub drm_vendors: Vec<String>,
    /// Windows: video controller names.
    pub video_controllers: Vec<String>,
}

/// GPU vendors with a working driver, from the probe output.
pub fn gpus_from(os: &str, p: &ProbeOutput) -> Vec<GpuVendor> {
    let mut out = Vec::new();
    let lower = |s: &Option<String>| s.as_deref().unwrap_or("").to_ascii_lowercase();
    let names: Vec<String> = p.video_controllers.iter().map(|n| n.to_ascii_lowercase()).collect();
    let name_has = |needles: &[&str]| names.iter().any(|n| needles.iter().any(|k| n.contains(k)));

    let nvidia_listed = p.nvidia_smi.as_deref().is_some_and(|s| s.lines().any(|l| l.trim_start().starts_with("GPU ")));
    if nvidia_listed || (os == "linux" && p.nvidia_proc) {
        out.push(GpuVendor::Nvidia);
    }

    let amd = match os {
        "linux" => lower(&p.rocminfo).contains("gfx") || p.rocm_smi || p.kfd,
        "windows" => name_has(&["radeon", "amd "]),
        _ => false,
    };
    if amd {
        out.push(GpuVendor::Amd);
    }

    let sycl_gpu = lower(&p.sycl_ls).lines().any(|l| l.contains("gpu") && l.contains("intel"))
        || lower(&p.xpu_smi).contains("device name");
    let intel = sycl_gpu
        || match os {
            "linux" => p.drm_vendors.iter().any(|v| v.trim().eq_ignore_ascii_case("0x8086")),
            "windows" => names.iter().any(|n| n.contains("intel") && n.contains("arc")),
            _ => false,
        };
    if intel {
        out.push(GpuVendor::Intel);
    }
    out
}

/// The hardware id for this probe output.
pub fn classify_probe(os: &str, arch: &str, p: &ProbeOutput) -> HostHardware {
    classify(os, arch, &gpus_from(os, p))
}

/// Why `hw` cannot be used now, given a fresh probe; `None` when it can.
pub fn device_unavailable(hw: Hardware, os: &str, arch: &str, p: &ProbeOutput) -> Option<String> {
    if matches!(hw, Hardware::Cpu | Hardware::AppleSilicon) {
        return None;
    }
    let now = classify_probe(os, arch, p).hardware;
    if now == hw {
        return None;
    }
    let what = match hw {
        Hardware::CudaLinux | Hardware::CudaWindows => {
            "nvidia-smi no longer lists a GPU and the NVIDIA driver does not answer"
        }
        Hardware::RocmLinux => "/dev/kfd is gone and rocminfo lists no AMD GPU, so the ROCm driver is not loaded",
        Hardware::XpuLinux | Hardware::XpuWindows => "no Intel GPU answers (sycl-ls, xpu-smi and the GPU driver found none)",
        Hardware::AppleSilicon | Hardware::Cpu => unreachable!(),
    };
    Some(format!(
        "This computer trains on {}, but that GPU cannot be used right now: {what}. \
         Check the GPU driver, or restart the computer, and try again.",
        hw.label()
    ))
}

// ── real probes ─────────────────────────────────────────────────────────

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Run a probe command; stdout when it exits 0 within the timeout. A missing
/// tool, a non-zero exit or a hang is `None`.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut out = String::new();
                use std::io::Read;
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return Some(out);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn drm_vendors() -> Vec<String> {
    let has_render = std::fs::read_dir("/dev/dri")
        .map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("renderD")))
        .unwrap_or(false);
    if !has_render {
        return vec![];
    }
    std::fs::read_dir("/sys/class/drm")
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with("card") && !n.contains('-')
                })
                .filter_map(|e| std::fs::read_to_string(e.path().join("device").join("vendor")).ok())
                .map(|v| v.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Probe this computer. Apple Silicon needs no probe (MLX/MPS are always there).
pub fn probe(os: &str, arch: &str) -> ProbeOutput {
    let mut p = ProbeOutput::default();
    match os {
        "linux" => {
            p.nvidia_proc = Path::new("/proc/driver/nvidia/version").exists();
            if !p.nvidia_proc {
                p.nvidia_smi = run("nvidia-smi", &["-L"]);
            }
            p.kfd = Path::new("/dev/kfd").exists();
            if !p.kfd {
                p.rocminfo = run("rocminfo", &[]);
                p.rocm_smi = p.rocminfo.is_none() && run("rocm-smi", &[]).is_some();
            }
            p.drm_vendors = drm_vendors();
            if !p.drm_vendors.iter().any(|v| v.eq_ignore_ascii_case("0x8086")) {
                p.sycl_ls = run("sycl-ls", &[]);
                if p.sycl_ls.is_none() {
                    p.xpu_smi = run("xpu-smi", &["discovery"]);
                }
            }
        }
        "windows" => {
            p.nvidia_smi = run("nvidia-smi", &["-L"]);
            p.video_controllers = run(
                "powershell",
                &["-NoProfile", "-NonInteractive", "-Command", "(Get-CimInstance Win32_VideoController).Name"],
            )
            .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
            .unwrap_or_default();
            p.sycl_ls = run("sycl-ls", &[]);
            if p.sycl_ls.is_none() {
                p.xpu_smi = run("xpu-smi", &["discovery"]);
            }
        }
        _ => {
            let _ = arch;
        }
    }
    p
}

/// This computer's training hardware, probed once per process.
pub fn detect() -> HostHardware {
    static CACHE: OnceLock<HostHardware> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
            let found = classify_probe(os, arch, &probe(os, arch));
            tracing::info!(hardware = found.hardware.id(), reason = ?found.reason, "studio: training hardware");
            found
        })
        .clone()
}

/// Checks at job start that the detected device still works (fresh probe,
/// not cached). `Err` is the reason for 503 `hardware_unavailable`.
pub type DeviceCheck = Arc<dyn Fn(Hardware) -> Result<(), String> + Send + Sync>;

pub fn real_device_check() -> DeviceCheck {
    Arc::new(|hw| {
        if matches!(hw, Hardware::Cpu | Hardware::AppleSilicon) {
            return Ok(());
        }
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        match device_unavailable(hw, os, arch, &probe(os, arch)) {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(os: &str, arch: &str, p: ProbeOutput) -> HostHardware {
        classify_probe(os, arch, &p)
    }

    #[test]
    fn probe_output_maps_to_every_id() {
        let none = ProbeOutput::default();
        assert_eq!(hw("macos", "aarch64", none.clone()).hardware, Hardware::AppleSilicon);
        let intel_mac = hw("macos", "x86_64", none.clone());
        assert_eq!(intel_mac.hardware, Hardware::Cpu);
        assert!(intel_mac.reason.as_deref().unwrap().contains("Intel Macs"));
        // No PyTorch 2.6+ on Intel Macs: the engine card refuses, not "LoRA runs".
        let tune = allternit_studio_spec::engines::find("allternit-tune").unwrap();
        let card = allternit_studio_spec::fit(&tune, &intel_mac);
        assert!(!card.ok);
        assert_eq!(card.reason, allternit_studio_spec::INTEL_MAC_NO_TORCH);

        let smi = ProbeOutput {
            nvidia_smi: Some("GPU 0: NVIDIA GeForce RTX 4090 (UUID: GPU-1234)\n".into()),
            ..Default::default()
        };
        assert_eq!(hw("linux", "x86_64", smi.clone()).hardware, Hardware::CudaLinux);
        assert_eq!(hw("windows", "x86_64", smi).hardware, Hardware::CudaWindows);
        // Driver loaded but nvidia-smi missing still counts on Linux.
        let proc_only = ProbeOutput { nvidia_proc: true, ..Default::default() };
        assert_eq!(hw("linux", "x86_64", proc_only).hardware, Hardware::CudaLinux);
        // nvidia-smi exiting 0 with no GPU line is not a GPU.
        let empty_smi = ProbeOutput { nvidia_smi: Some("No devices found.\n".into()), ..Default::default() };
        assert_eq!(hw("linux", "x86_64", empty_smi).hardware, Hardware::Cpu);

        let rocm = ProbeOutput {
            rocminfo: Some("Agent 2\n  Name:                    gfx1100\n".into()),
            drm_vendors: vec!["0x1002".into(), "0x8086".into()],
            ..Default::default()
        };
        // AMD wins over an Intel iGPU next to it.
        assert_eq!(hw("linux", "x86_64", rocm).hardware, Hardware::RocmLinux);
        let kfd = ProbeOutput { kfd: true, ..Default::default() };
        assert_eq!(hw("linux", "x86_64", kfd).hardware, Hardware::RocmLinux);

        let intel_drm = ProbeOutput { drm_vendors: vec!["0x8086\n".into()], ..Default::default() };
        assert_eq!(hw("linux", "x86_64", intel_drm).hardware, Hardware::XpuLinux);
        let sycl = ProbeOutput {
            sycl_ls: Some("[level_zero:gpu][level_zero:0] Intel(R) Level-Zero, Intel(R) Arc(TM) A770 Graphics 1.3\n".into()),
            ..Default::default()
        };
        assert_eq!(hw("windows", "x86_64", sycl).hardware, Hardware::XpuWindows);
        let arc = ProbeOutput { video_controllers: vec!["Intel(R) Arc(TM) A750 Graphics".into()], ..Default::default() };
        assert_eq!(hw("windows", "x86_64", arc).hardware, Hardware::XpuWindows);
        // Plain Intel UHD on Windows is not a training GPU.
        let uhd = ProbeOutput { video_controllers: vec!["Intel(R) UHD Graphics 770".into()], ..Default::default() };
        assert_eq!(hw("windows", "x86_64", uhd).hardware, Hardware::Cpu);

        let radeon = hw(
            "windows",
            "x86_64",
            ProbeOutput { video_controllers: vec!["AMD Radeon RX 7900 XTX".into()], ..Default::default() },
        );
        assert_eq!(radeon.hardware, Hardware::Cpu);
        assert!(radeon.reason.unwrap().contains("AMD GPUs on Windows"));

        let bare = hw("linux", "x86_64", none);
        assert_eq!(bare.hardware, Hardware::Cpu);
        assert!(bare.reason.unwrap().contains("No NVIDIA, AMD or Intel GPU"));
    }

    #[test]
    fn device_unavailable_when_the_gpu_went_away() {
        let gone = ProbeOutput::default();
        let why = device_unavailable(Hardware::CudaLinux, "linux", "x86_64", &gone).unwrap();
        assert!(why.contains("Linux with an NVIDIA GPU") && why.contains("nvidia-smi"), "{why}");
        assert!(device_unavailable(Hardware::RocmLinux, "linux", "x86_64", &gone).unwrap().contains("/dev/kfd"));
        let ok = ProbeOutput { kfd: true, ..Default::default() };
        assert_eq!(device_unavailable(Hardware::RocmLinux, "linux", "x86_64", &ok), None);
        assert_eq!(device_unavailable(Hardware::Cpu, "linux", "x86_64", &gone), None);
    }

    #[test]
    fn missing_probe_tool_is_not_an_error() {
        assert_eq!(run("allternit-no-such-probe-tool", &[]), None);
    }
}
