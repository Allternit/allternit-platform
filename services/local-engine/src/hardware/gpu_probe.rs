//! GPU probes shared by the hardware profile and `/status`.
//!
//! NVIDIA: `nvidia-smi` (Linux and Windows; the Windows driver installs it
//! into System32). Windows also lists every display adapter through WMI
//! (`Win32_VideoController`), so AMD/Intel GPUs show up even without
//! nvidia-smi. Parsers are pure so they're tested on every OS.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedGpu {
    pub name: String,
    pub memory_total_mb: Option<u64>,
    pub memory_used_mb: Option<u64>,
}

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

/// Parse `Name|AdapterRAM` lines from `Get-CimInstance Win32_VideoController`.
/// AdapterRAM is a 32-bit field: cards with 4 GB or more report a capped
/// value, so anything at the cap is "unknown" rather than a wrong 4 GB.
/// Virtual/remote adapters are skipped.
pub fn parse_video_controllers(text: &str) -> Vec<ProbedGpu> {
    const CAPPED: u64 = 0xFFF0_0000;
    text.lines()
        .filter_map(|line| {
            let (name, ram) = line.trim().split_once('|').unwrap_or((line.trim(), ""));
            let name = name.trim();
            let lower = name.to_ascii_lowercase();
            if name.is_empty()
                || lower.contains("basic display")
                || lower.contains("basic render")
                || lower.contains("remote display")
                || lower.contains("indirect display")
            {
                return None;
            }
            let bytes: Option<u64> = ram.trim().parse().ok();
            Some(ProbedGpu {
                name: name.to_string(),
                memory_total_mb: bytes.filter(|b| *b > 0 && *b < CAPPED).map(|b| b / (1024 * 1024)),
                memory_used_mb: None,
            })
        })
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn quiet_command(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// NVIDIA GPUs via nvidia-smi; empty when it's missing or fails.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub fn nvidia_smi() -> Vec<ProbedGpu> {
    match quiet_command("nvidia-smi")
        .args(["--query-gpu=name,memory.total,memory.used", "--format=csv,noheader,nounits"])
        .output()
    {
        Ok(out) if out.status.success() => parse_nvidia_smi(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

/// Display adapters via WMI; empty when PowerShell/WMI is unavailable.
#[cfg(target_os = "windows")]
pub fn video_controllers() -> Vec<ProbedGpu> {
    let script = "Get-CimInstance Win32_VideoController | ForEach-Object { \"$($_.Name)|$($_.AdapterRAM)\" }";
    match quiet_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    {
        Ok(out) if out.status.success() => parse_video_controllers(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
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
    }

    #[test]
    fn parses_wmi_adapters_and_skips_virtual_ones() {
        let text = "AMD Radeon RX 6600|4293918720\r\nIntel(R) UHD Graphics 770|2147483648\r\nMicrosoft Basic Display Adapter|0\r\nMicrosoft Remote Display Adapter|\r\n";
        let gpus = parse_video_controllers(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0], ProbedGpu { name: "AMD Radeon RX 6600".into(), memory_total_mb: None, memory_used_mb: None });
        assert_eq!(gpus[1].memory_total_mb, Some(2048));
    }
}
