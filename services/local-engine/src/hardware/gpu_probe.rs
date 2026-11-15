//! GPU probes shared by the hardware profile, `/status` and `/hardware`.
//!
//! - macOS: Apple silicon is one unified-memory GPU named after the chip
//!   (`sysctl`, no system_profiler). Intel Macs fall back to
//!   `system_profiler SPDisplaysDataType -json`.
//! - Linux: `nvidia-smi` for NVIDIA, then `/sys/class/drm/card*/device` for
//!   AMD (`mem_info_vram_total` / `mem_info_vram_used`), Intel, Qualcomm
//!   (`msm`) and Apple (`asahi`). Names come from `pci.ids` when installed.
//! - Windows: `nvidia-smi` (the driver installs it into System32), plus every
//!   adapter WMI lists (`Win32_VideoController`). WMI's `AdapterRAM` is a
//!   32-bit field that caps at 4 GB, so the 64-bit
//!   `HardwareInformation.qwMemorySize` from the display class registry key
//!   wins when it is there.
//!
//! Every external command runs with a timeout ([`PROBE_TIMEOUT`]) and every
//! probe is best-effort: a failure means fewer GPUs, never an error or panic.
//! Parsers are pure so they are tested on every OS.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest any single probe command may run.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedGpu {
    pub name: String,
    pub memory_total_mb: Option<u64>,
    pub memory_used_mb: Option<u64>,
}

/// GPU vendor as reported by `GET /hardware`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Qualcomm,
    Other,
}

/// Where a GPU came from, so the free-memory refresh can re-read only it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GpuSource {
    #[default]
    Static,
    /// Apple silicon: GPU memory is system RAM.
    AppleUnified,
    /// Row index in `nvidia-smi` output.
    NvidiaSmi(usize),
    /// `/sys/class/drm/cardN/device` directory (AMD VRAM counters).
    Sysfs(PathBuf),
}

/// One GPU entry of `GET /hardware`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HardwareGpu {
    pub name: String,
    pub vendor: GpuVendor,
    pub vram_total_bytes: Option<u64>,
    pub vram_free_bytes: Option<u64>,
    pub discrete: bool,
    #[serde(skip)]
    pub source: GpuSource,
}

// ─── Timed command runner ───────────────────────────────────────────────────

/// Output of a command that finished within its timeout.
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `cmd` and wait at most `timeout`. Returns `None` when it can't start
/// or runs too long (the child is killed). Pipes are drained on threads so a
/// chatty child can't deadlock, and a grandchild holding a pipe open can't
/// hang the caller past the deadline.
pub fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<CmdOutput> {
    use std::io::Read;
    use std::sync::mpsc;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let deadline = Instant::now() + timeout;

    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    if let Some(mut out) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send((true, buf));
        });
    }
    if let Some(mut err) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            let _ = tx.send((false, buf));
        });
    }
    drop(tx);

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };

    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(200));
        match rx.recv_timeout(wait) {
            Ok((true, buf)) => stdout = buf,
            Ok((false, buf)) => stderr = buf,
            Err(_) => break,
        }
    }
    Some(CmdOutput {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// A command that opens no console window on Windows.
pub fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Stdout of a successful, timely run; `None` otherwise.
fn run_stdout(program: &str, args: &[&str]) -> Option<String> {
    let out = run_with_timeout(quiet_command(program).args(args), PROBE_TIMEOUT)?;
    out.success.then_some(out.stdout)
}

// ─── Parsers (pure) ─────────────────────────────────────────────────────────

/// Parse `nvidia-smi --query-gpu=name,memory.total,memory.used --format=csv,noheader,nounits`.
pub fn parse_nvidia_smi(text: &str) -> Vec<ProbedGpu> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').map(str::trim).collect();
            let name = parts.first().copied().unwrap_or("");
            (!name.is_empty()).then(|| ProbedGpu {
                name: name.to_string(),
                memory_total_mb: parts.get(1).and_then(|s| s.parse().ok()),
                memory_used_mb: parts.get(2).and_then(|s| s.parse().ok()),
            })
        })
        .collect()
}

/// Parse `Name|AdapterRAM[|qwMemorySize]` lines built from
/// `Win32_VideoController` (and the display class registry key).
/// AdapterRAM is a 32-bit field: cards with 4 GB or more report a capped
/// value, so anything at the cap is "unknown" rather than a wrong 4 GB. The
/// optional 64-bit `qwMemorySize` wins when present. Virtual/remote adapters
/// are skipped.
pub fn parse_video_controllers(text: &str) -> Vec<ProbedGpu> {
    const CAPPED: u64 = 0xFFF0_0000;
    text.lines()
        .filter_map(|line| {
            let mut fields = line.trim().split('|');
            let name = fields.next().unwrap_or("").trim();
            let adapter_ram: Option<u64> = fields.next().and_then(|s| s.trim().parse().ok());
            let qw_memory: Option<u64> = fields.next().and_then(|s| s.trim().parse().ok());
            let lower = name.to_ascii_lowercase();
            if name.is_empty()
                || lower.contains("basic display")
                || lower.contains("basic render")
                || lower.contains("remote display")
                || lower.contains("indirect display")
            {
                return None;
            }
            let bytes = qw_memory
                .filter(|b| *b > 0)
                .or_else(|| adapter_ram.filter(|b| *b > 0 && *b < CAPPED));
            Some(ProbedGpu {
                name: name.to_string(),
                memory_total_mb: bytes.map(|b| b / MIB),
                memory_used_mb: None,
            })
        })
        .collect()
}

/// Vendor from a marketing name ("NVIDIA GeForce RTX 4090", "AMD Radeon RX
/// 7900 XTX", "Intel(R) Arc(TM) A770", "Qualcomm(R) Adreno(TM) X1-85 GPU").
pub fn vendor_from_name(name: &str) -> GpuVendor {
    let l = name.to_ascii_lowercase();
    if ["nvidia", "geforce", "quadro", "tesla"].iter().any(|k| l.contains(k)) {
        GpuVendor::Nvidia
    } else if ["amd", "radeon", "instinct", "firepro"].iter().any(|k| l.contains(k)) {
        GpuVendor::Amd
    } else if l.contains("intel") || l.contains("arc(tm)") {
        GpuVendor::Intel
    } else if l.contains("apple") {
        GpuVendor::Apple
    } else if l.contains("qualcomm") || l.contains("adreno") {
        GpuVendor::Qualcomm
    } else {
        GpuVendor::Other
    }
}

/// Whether a GPU name is a discrete card. NVIDIA is always discrete. AMD
/// integrated parts are named "... Graphics" ("Radeon 780M Graphics",
/// "Radeon RX Vega 11 Graphics") or by APU codename in pci.ids ("Phoenix1");
/// discrete ones carry RX / Pro / FirePro / Instinct. Intel is discrete only
/// for Arc A-/B-series model numbers ("Arc(TM) Graphics" and "Arc 140V" are
/// integrated). Apple and Qualcomm GPUs are integrated.
pub fn looks_discrete(vendor: GpuVendor, name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    match vendor {
        GpuVendor::Nvidia => true,
        GpuVendor::Amd => {
            if l.trim_end().ends_with("graphics") {
                return false;
            }
            ["rx ", "radeon pro", "firepro", "instinct", "radeon vii"]
                .iter()
                .any(|k| l.contains(k))
        }
        GpuVendor::Intel => regex::Regex::new(r"(?i)\barc(\(tm\))?\s+(pro\s+)?[ab]\d{2,3}")
            .map(|re| re.is_match(name))
            .unwrap_or(false),
        GpuVendor::Apple | GpuVendor::Qualcomm | GpuVendor::Other => false,
    }
}

/// Look up a PCI device name in `pci.ids` text. Vendor lines start at
/// column 0, device lines with one tab, subsystem lines with two tabs.
pub fn pci_ids_lookup(ids: &str, vendor: u16, device: u16) -> Option<String> {
    let vendor_prefix = format!("{vendor:04x}  ");
    let device_prefix = format!("\t{device:04x}  ");
    let mut in_vendor = false;
    for line in ids.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return None;
            }
            in_vendor = line.starts_with(&vendor_prefix);
            continue;
        }
        if in_vendor && line.starts_with(&device_prefix) {
            return Some(line[device_prefix.len()..].trim().to_string());
        }
    }
    None
}

fn parse_hex_u16(text: &str) -> Option<u16> {
    u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn uevent_driver(dir: &Path) -> Option<String> {
    read_trim(&dir.join("uevent"))?
        .lines()
        .find_map(|l| l.strip_prefix("DRIVER=").map(str::to_string))
}

/// Intel discrete device IDs: Alchemist (DG2, 0x56xx) and Battlemage
/// (0xe20x / 0xe21x).
fn intel_discrete_id(device: u16) -> bool {
    device >> 8 == 0x56 || device & 0xfff0 == 0xe200 || device & 0xfff0 == 0xe210
}

/// Probe `<drm_root>/card*/device` (normally `/sys/class/drm`). NVIDIA cards
/// are skipped when `skip_nvidia` (nvidia-smi already reported them).
/// `pci_ids` is the text of `pci.ids`, if any, for marketing names.
pub fn probe_drm(drm_root: &Path, skip_nvidia: bool, pci_ids: Option<&str>) -> Vec<HardwareGpu> {
    let Ok(entries) = std::fs::read_dir(drm_root) else {
        return Vec::new();
    };
    let mut cards: Vec<(u32, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n: u32 = name.strip_prefix("card")?.parse().ok()?;
            Some((n, e.path().join("device")))
        })
        .collect();
    cards.sort_by_key(|(n, _)| *n);

    let mut gpus = Vec::new();
    for (_, dev) in cards {
        let vendor_id = read_trim(&dev.join("vendor")).and_then(|s| parse_hex_u16(&s));
        let device_id = read_trim(&dev.join("device")).and_then(|s| parse_hex_u16(&s));
        let driver = uevent_driver(&dev).unwrap_or_default();
        let pci_name = match (vendor_id, device_id, pci_ids) {
            (Some(v), Some(d), Some(ids)) => pci_ids_lookup(ids, v, d),
            _ => None,
        };
        let id_label = |label: &str| match (vendor_id, device_id) {
            (Some(v), Some(d)) => format!("{label} ({v:04x}:{d:04x})"),
            _ => label.to_string(),
        };

        let gpu = match vendor_id {
            Some(0x10de) => {
                if skip_nvidia {
                    continue;
                }
                HardwareGpu {
                    name: pci_name.unwrap_or_else(|| id_label("NVIDIA GPU")),
                    vendor: GpuVendor::Nvidia,
                    vram_total_bytes: None,
                    vram_free_bytes: None,
                    discrete: true,
                    source: GpuSource::Static,
                }
            }
            Some(0x1002) => {
                let total = read_trim(&dev.join("mem_info_vram_total")).and_then(|s| s.parse::<u64>().ok());
                let used = read_trim(&dev.join("mem_info_vram_used")).and_then(|s| s.parse::<u64>().ok());
                let discrete = match &pci_name {
                    Some(name) => looks_discrete(GpuVendor::Amd, name),
                    // No name: an APU's carve-out is usually under 4 GB.
                    None => total.is_some_and(|t| t >= 4 * GIB),
                };
                HardwareGpu {
                    name: pci_name.unwrap_or_else(|| id_label("AMD GPU")),
                    vendor: GpuVendor::Amd,
                    vram_total_bytes: total.filter(|t| *t > 0),
                    vram_free_bytes: total.zip(used).map(|(t, u)| t.saturating_sub(u)),
                    discrete,
                    source: GpuSource::Sysfs(dev.clone()),
                }
            }
            Some(0x8086) => {
                let discrete = device_id.is_some_and(intel_discrete_id)
                    || pci_name.as_deref().is_some_and(|n| looks_discrete(GpuVendor::Intel, n));
                HardwareGpu {
                    name: pci_name.unwrap_or_else(|| id_label("Intel GPU")),
                    vendor: GpuVendor::Intel,
                    vram_total_bytes: None,
                    vram_free_bytes: None,
                    discrete,
                    source: GpuSource::Static,
                }
            }
            _ if driver.starts_with("msm") => HardwareGpu {
                name: "Qualcomm Adreno".into(),
                vendor: GpuVendor::Qualcomm,
                vram_total_bytes: None,
                vram_free_bytes: None,
                discrete: false,
                source: GpuSource::Static,
            },
            _ if driver == "asahi" => HardwareGpu {
                name: "Apple GPU".into(),
                vendor: GpuVendor::Apple,
                vram_total_bytes: None,
                vram_free_bytes: None,
                discrete: false,
                source: GpuSource::Static,
            },
            // simpledrm, virtio, vc4 and other display-only devices.
            _ => continue,
        };
        gpus.push(gpu);
    }
    gpus
}

/// Parse a macOS VRAM string ("8 GB", "1536 MB").
fn parse_mac_vram(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace();
    let n: f64 = parts.next()?.parse().ok()?;
    let unit = parts.next().unwrap_or("MB").to_ascii_uppercase();
    let mult = match unit.as_str() {
        "GB" => GIB,
        "MB" => MIB,
        _ => return None,
    };
    Some((n * mult as f64) as u64)
}

/// Parse `system_profiler SPDisplaysDataType -json` (Intel Macs).
pub fn parse_system_profiler(text: &str) -> Vec<HardwareGpu> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(arr) = json.get("SPDisplaysDataType").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|d| {
            let name = d["sppci_model"].as_str().or_else(|| d["_name"].as_str())?.to_string();
            let vendor_key = d["spdisplays_vendor"].as_str().unwrap_or("").to_ascii_lowercase();
            let vendor = if vendor_key.contains("amd") || vendor_key.contains("ati") {
                GpuVendor::Amd
            } else if vendor_key.contains("nvidia") {
                GpuVendor::Nvidia
            } else if vendor_key.contains("intel") {
                GpuVendor::Intel
            } else {
                vendor_from_name(&name)
            };
            let dedicated = ["spdisplays_vram", "_spdisplays_vram", "sppci_vram"]
                .iter()
                .find_map(|k| d[*k].as_str())
                .and_then(parse_mac_vram);
            let bus_pcie = d["sppci_bus"].as_str().is_some_and(|b| b.contains("pcie"));
            let discrete = bus_pcie || (dedicated.is_some() && looks_discrete(vendor, &name));
            Some(HardwareGpu {
                name,
                vendor,
                vram_total_bytes: if discrete { dedicated } else { None },
                vram_free_bytes: None,
                discrete,
                source: GpuSource::Static,
            })
        })
        .collect()
}

// ─── Probes (best-effort, timed) ────────────────────────────────────────────

/// NVIDIA GPUs via nvidia-smi; empty when it's missing, fails or hangs.
pub fn nvidia_smi() -> Vec<ProbedGpu> {
    run_stdout(
        "nvidia-smi",
        &["--query-gpu=name,memory.total,memory.used", "--format=csv,noheader,nounits"],
    )
    .map(|text| parse_nvidia_smi(&text))
    .unwrap_or_default()
}

/// Display adapters via WMI; empty when PowerShell/WMI is unavailable.
#[cfg(target_os = "windows")]
pub fn video_controllers() -> Vec<ProbedGpu> {
    // AdapterRAM caps at 4 GB; the display class key's 64-bit
    // HardwareInformation.qwMemorySize has the real size, keyed by DriverDesc.
    let script = concat!(
        "$reg=@{}; ",
        "Get-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0*' -ErrorAction SilentlyContinue | ",
        "ForEach-Object { if ($_.DriverDesc) { $reg[$_.DriverDesc] = $_.'HardwareInformation.qwMemorySize' } }; ",
        "Get-CimInstance Win32_VideoController | ForEach-Object { \"$($_.Name)|$($_.AdapterRAM)|$($reg[$_.Name])\" }"
    );
    run_stdout("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", script])
        .map(|text| parse_video_controllers(&text))
        .unwrap_or_default()
}

/// Windows GPUs: nvidia-smi when present (exact VRAM and usage), else WMI.
#[cfg(target_os = "windows")]
pub fn windows_gpus() -> Vec<ProbedGpu> {
    let nvidia = nvidia_smi();
    if !nvidia.is_empty() {
        return nvidia;
    }
    video_controllers()
}

pub fn nvidia_to_hardware(gpus: Vec<ProbedGpu>) -> Vec<HardwareGpu> {
    gpus.into_iter()
        .enumerate()
        .map(|(i, g)| HardwareGpu {
            vendor: GpuVendor::Nvidia,
            vram_total_bytes: g.memory_total_mb.map(|mb| mb * MIB),
            vram_free_bytes: g
                .memory_total_mb
                .zip(g.memory_used_mb)
                .map(|(t, u)| t.saturating_sub(u) * MIB),
            discrete: true,
            source: GpuSource::NvidiaSmi(i),
            name: g.name,
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn read_pci_ids() -> Option<String> {
    ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
}

/// Apple silicon chip name from `machdep.cpu.brand_string` ("Apple M1 Pro"),
/// when this Mac is Apple silicon (`hw.optional.arm64`, true under Rosetta too).
#[cfg(target_os = "macos")]
pub fn apple_silicon_brand() -> Option<String> {
    let arm = run_stdout("sysctl", &["-n", "hw.optional.arm64"])?;
    if arm.trim() != "1" {
        return None;
    }
    let brand = run_stdout("sysctl", &["-n", "machdep.cpu.brand_string"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Apple silicon".to_string());
    Some(brand)
}

/// Every GPU on this machine. `ram_total` / `ram_available` back the Apple
/// unified GPU's memory. Never fails: errors yield an empty list.
pub fn probe_gpus(ram_total: u64, ram_available: u64) -> Vec<HardwareGpu> {
    #[cfg(target_os = "macos")]
    {
        if let Some(brand) = apple_silicon_brand() {
            return vec![HardwareGpu {
                name: brand,
                vendor: GpuVendor::Apple,
                vram_total_bytes: Some(ram_total),
                vram_free_bytes: Some(ram_available),
                discrete: false,
                source: GpuSource::AppleUnified,
            }];
        }
        run_stdout("system_profiler", &["SPDisplaysDataType", "-json"])
            .map(|text| parse_system_profiler(&text))
            .unwrap_or_default()
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (ram_total, ram_available);
        let mut gpus = nvidia_to_hardware(nvidia_smi());
        let skip_nvidia = !gpus.is_empty();
        let ids = read_pci_ids();
        gpus.extend(probe_drm(Path::new("/sys/class/drm"), skip_nvidia, ids.as_deref()));
        gpus
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (ram_total, ram_available);
        let mut gpus = nvidia_to_hardware(nvidia_smi());
        let have_nvidia = !gpus.is_empty();
        for g in video_controllers() {
            let vendor = vendor_from_name(&g.name);
            if have_nvidia && vendor == GpuVendor::Nvidia {
                continue;
            }
            let discrete = looks_discrete(vendor, &g.name);
            gpus.push(HardwareGpu {
                vram_total_bytes: if discrete { g.memory_total_mb.map(|mb| mb * MIB) } else { None },
                vram_free_bytes: None,
                discrete,
                vendor,
                source: GpuSource::Static,
                name: g.name,
            });
        }
        gpus
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (ram_total, ram_available);
        Vec::new()
    }
}

/// Re-read only the free-memory counters (cheap; used between full probes).
pub fn refresh_vram_free(gpus: &mut [HardwareGpu], ram_available: u64) {
    let nvidia = if gpus.iter().any(|g| matches!(g.source, GpuSource::NvidiaSmi(_))) {
        Some(nvidia_smi())
    } else {
        None
    };
    for gpu in gpus.iter_mut() {
        match &gpu.source {
            GpuSource::AppleUnified => gpu.vram_free_bytes = Some(ram_available),
            GpuSource::NvidiaSmi(i) => {
                if let Some(row) = nvidia.as_ref().and_then(|rows| rows.get(*i)) {
                    gpu.vram_free_bytes = row
                        .memory_total_mb
                        .zip(row.memory_used_mb)
                        .map(|(t, u)| t.saturating_sub(u) * MIB);
                }
            }
            GpuSource::Sysfs(dev) => {
                let used = read_trim(&dev.join("mem_info_vram_used")).and_then(|s| s.parse::<u64>().ok());
                if let (Some(total), Some(used)) = (gpu.vram_total_bytes, used) {
                    gpu.vram_free_bytes = Some(total.saturating_sub(used));
                }
            }
            GpuSource::Static => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi_rows() {
        let gpus = parse_nvidia_smi("NVIDIA GeForce RTX 4090, 24564, 1200\nNVIDIA RTX A2000, 6138, [N/A]\n\n");
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(gpus[0].memory_total_mb, Some(24564));
        assert_eq!(gpus[0].memory_used_mb, Some(1200));
        assert_eq!(gpus[1].memory_used_mb, None);

        let hw = nvidia_to_hardware(gpus);
        assert_eq!(hw[0].vram_total_bytes, Some(24564 * MIB));
        assert_eq!(hw[0].vram_free_bytes, Some((24564 - 1200) * MIB));
        assert_eq!(hw[1].vram_free_bytes, None, "unknown usage is not 'all free'");
        assert_eq!(hw[1].source, GpuSource::NvidiaSmi(1));
    }

    #[test]
    fn parses_wmi_adapters_and_skips_virtual_ones() {
        let text = "AMD Radeon RX 6600|4293918720\r\nIntel(R) UHD Graphics 770|2147483648\r\nMicrosoft Basic Display Adapter|0\r\nMicrosoft Remote Display Adapter|\r\n";
        let gpus = parse_video_controllers(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0], ProbedGpu { name: "AMD Radeon RX 6600".into(), memory_total_mb: None, memory_used_mb: None });
        assert_eq!(gpus[1].memory_total_mb, Some(2048));
    }

    #[test]
    fn wmi_registry_size_beats_capped_adapter_ram() {
        // 24 GB card: AdapterRAM capped, qwMemorySize exact.
        let gpus = parse_video_controllers("AMD Radeon RX 7900 XTX|4293918720|25753026560\r\nIntel(R) Arc(TM) Graphics|2147483648|\r\n");
        assert_eq!(gpus[0].memory_total_mb, Some(25753026560 / MIB));
        assert_eq!(gpus[1].memory_total_mb, Some(2048), "empty registry field falls back to AdapterRAM");
    }

    #[test]
    fn classifies_discrete_vs_integrated_names() {
        let cases = [
            ("NVIDIA GeForce RTX 3060 Laptop GPU", GpuVendor::Nvidia, true),
            ("AMD Radeon RX 7900 XTX", GpuVendor::Amd, true),
            ("AMD Radeon(TM) 780M Graphics", GpuVendor::Amd, false),
            ("AMD Radeon RX Vega 11 Graphics", GpuVendor::Amd, false),
            ("Navi 31 [Radeon RX 7900 XT/7900 XTX/7900M]", GpuVendor::Amd, true),
            ("Phoenix1", GpuVendor::Other, false),
            ("Intel(R) Arc(TM) A770 Graphics", GpuVendor::Intel, true),
            ("Intel(R) Arc(TM) B580 Graphics", GpuVendor::Intel, true),
            ("Intel(R) Arc(TM) Graphics", GpuVendor::Intel, false),
            ("Intel(R) Arc(TM) 140V GPU (16GB)", GpuVendor::Intel, false),
            ("Qualcomm(R) Adreno(TM) X1-85 GPU", GpuVendor::Qualcomm, false),
        ];
        for (name, vendor, discrete) in cases {
            assert_eq!(vendor_from_name(name), vendor, "{name}");
            assert_eq!(looks_discrete(vendor, name), discrete, "{name}");
        }
    }

    #[test]
    fn pci_ids_lookup_stays_inside_the_vendor_block() {
        let ids = "# comment\n1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t15bf  Phoenix1\n\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900M]\n\t\t1002 0e3b  RX 7900 GRE\n10de  NVIDIA Corporation\n\t744c  Not this one\n";
        assert_eq!(pci_ids_lookup(ids, 0x1002, 0x744c).as_deref(), Some("Navi 31 [Radeon RX 7900 XT/7900 XTX/7900M]"));
        assert_eq!(pci_ids_lookup(ids, 0x1002, 0x15bf).as_deref(), Some("Phoenix1"));
        assert_eq!(pci_ids_lookup(ids, 0x1002, 0x0e3b), None, "subsystem lines are not devices");
        assert_eq!(pci_ids_lookup(ids, 0x10de, 0x744c).as_deref(), Some("Not this one"));
    }

    fn card(root: &Path, n: u32, files: &[(&str, &str)]) {
        let dev = root.join(format!("card{n}")).join("device");
        std::fs::create_dir_all(&dev).unwrap();
        for (name, body) in files {
            std::fs::write(dev.join(name), body).unwrap();
        }
    }

    #[test]
    fn probes_drm_sysfs_for_amd_intel_and_platform_gpus() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Discrete AMD with VRAM counters.
        card(root, 0, &[
            ("vendor", "0x1002\n"), ("device", "0x744c\n"), ("uevent", "DRIVER=amdgpu\n"),
            ("mem_info_vram_total", "25753026560\n"), ("mem_info_vram_used", "1073741824\n"),
        ]);
        // AMD APU: 512 MB carve-out, no pci.ids name for it.
        card(root, 1, &[
            ("vendor", "0x1002\n"), ("device", "0x15bf\n"), ("uevent", "DRIVER=amdgpu\n"),
            ("mem_info_vram_total", "536870912\n"), ("mem_info_vram_used", "100\n"),
        ]);
        // Intel Arc A770 (DG2) and an Intel iGPU.
        card(root, 2, &[("vendor", "0x8086\n"), ("device", "0x56a0\n"), ("uevent", "DRIVER=i915\n")]);
        card(root, 3, &[("vendor", "0x8086\n"), ("device", "0xa780\n"), ("uevent", "DRIVER=i915\n")]);
        // NVIDIA (skipped: nvidia-smi reported it), Snapdragon, simpledrm.
        card(root, 4, &[("vendor", "0x10de\n"), ("device", "0x2684\n"), ("uevent", "DRIVER=nvidia\n")]);
        card(root, 5, &[("uevent", "DRIVER=msm_dpu\nOF_NAME=display-subsystem\n")]);
        card(root, 6, &[("uevent", "DRIVER=simpledrm\n")]);
        // Connector and render nodes are not cards.
        std::fs::create_dir_all(root.join("card0-DP-1")).unwrap();
        std::fs::create_dir_all(root.join("renderD128")).unwrap();

        let ids = "1002  AMD\n\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900M]\n";
        let gpus = probe_drm(root, true, Some(ids));
        let names: Vec<&str> = gpus.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, [
            "Navi 31 [Radeon RX 7900 XT/7900 XTX/7900M]",
            "AMD GPU (1002:15bf)",
            "Intel GPU (8086:56a0)",
            "Intel GPU (8086:a780)",
            "Qualcomm Adreno",
        ]);
        assert!(gpus[0].discrete);
        assert_eq!(gpus[0].vram_total_bytes, Some(25753026560));
        assert_eq!(gpus[0].vram_free_bytes, Some(25753026560 - 1073741824));
        assert!(!gpus[1].discrete, "small carve-out without a name is an APU");
        assert!(gpus[2].discrete && !gpus[3].discrete);
        assert_eq!(gpus[4].vendor, GpuVendor::Qualcomm);

        let with_nvidia = probe_drm(root, false, None);
        assert!(with_nvidia.iter().any(|g| g.vendor == GpuVendor::Nvidia && g.discrete && g.vram_total_bytes.is_none()));
        assert!(probe_drm(&root.join("missing"), false, None).is_empty());
    }

    #[test]
    fn parses_intel_mac_system_profiler() {
        let json = r#"{"SPDisplaysDataType":[
            {"_name":"Intel UHD Graphics 630","sppci_model":"Intel UHD Graphics 630","sppci_bus":"spdisplays_builtin","spdisplays_vram":"1536 MB","spdisplays_vendor":"Intel"},
            {"_name":"AMD Radeon Pro 5500M","sppci_model":"AMD Radeon Pro 5500M","sppci_bus":"spdisplays_pcie_device","spdisplays_vram":"8 GB","spdisplays_vendor":"sppci_vendor_amd"}]}"#;
        let gpus = parse_system_profiler(json);
        assert_eq!(gpus.len(), 2);
        assert!(!gpus[0].discrete);
        assert_eq!(gpus[0].vram_total_bytes, None);
        assert_eq!(gpus[1].vendor, GpuVendor::Amd);
        assert!(gpus[1].discrete);
        assert_eq!(gpus[1].vram_total_bytes, Some(8 * GIB));
    }

    #[test]
    fn timed_runner_kills_slow_commands() {
        #[cfg(unix)]
        {
            let start = Instant::now();
            assert!(run_with_timeout(Command::new("sleep").arg("5"), Duration::from_millis(300)).is_none());
            assert!(start.elapsed() < Duration::from_secs(2));
            let out = run_with_timeout(Command::new("sh").args(["-c", "echo out; echo err >&2"]), PROBE_TIMEOUT).unwrap();
            assert!(out.success);
            assert_eq!(out.stdout.trim(), "out");
            assert_eq!(out.stderr.trim(), "err");
            assert!(run_with_timeout(&mut Command::new("/definitely/not/here"), PROBE_TIMEOUT).is_none());
        }
    }
}
