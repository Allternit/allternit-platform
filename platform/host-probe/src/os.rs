//! OS identity (name, version, kernel, arch, hostname) and the primary-disk
//! rule. The parsers here are pure so they can be tested on any host.

use std::collections::HashMap;

use sysinfo::System;

use crate::{OsFamily, OsInfo, ProbeError, Result};

/// Parse an os-release file (`/etc/os-release`) into KEY -> value.
///
/// Follows os-release(5): one `KEY=value` per line, `#` comments, values
/// bare, single-quoted, or double-quoted with `\"`, `\\`, `\$` and `` \` ``
/// escapes. Lines that are not assignments are ignored.
pub fn parse_os_release(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        out.insert(key.to_string(), unquote(value.trim()));
    }
    out
}

fn unquote(v: &str) -> String {
    let bytes = v.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        return v[1..v.len() - 1].to_string();
    }
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        let inner = &v[1..v.len() - 1];
        let mut s = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some(n @ ('"' | '\\' | '$' | '`')) => s.push(n),
                    Some(n) => {
                        s.push('\\');
                        s.push(n);
                    }
                    None => s.push('\\'),
                }
            } else {
                s.push(c);
            }
        }
        return s;
    }
    v.to_string()
}

/// Display name and version from parsed os-release fields:
/// name = `PRETTY_NAME`, else `NAME`, else "Linux"; version = `VERSION_ID`,
/// else `VERSION`, else empty.
pub fn linux_name_version(fields: &HashMap<String, String>) -> (String, String) {
    let pick = |keys: &[&str]| {
        keys.iter()
            .filter_map(|k| fields.get(*k))
            .find(|v| !v.trim().is_empty())
            .cloned()
    };
    let name = pick(&["PRETTY_NAME", "NAME"]).unwrap_or_else(|| "Linux".to_string());
    let version = pick(&["VERSION_ID", "VERSION"]).unwrap_or_default();
    (name, version)
}

/// Normalize an architecture name so every OS reports the same words:
/// `aarch64`/`arm64` -> `arm64`, `amd64`/`x86_64`/`x64` -> `x86_64`.
/// Anything else is returned lowercased.
pub fn normalize_arch(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "aarch64" | "arm64" => "arm64".to_string(),
        "x86_64" | "amd64" | "x64" => "x86_64".to_string(),
        other => other.to_string(),
    }
}

/// Index of the primary (system) volume in `mounts`.
///
/// Linux and macOS: the volume mounted at `/`. Windows: the volume whose
/// mount matches `system_drive` (the `SystemDrive` env value, for example
/// `C:`), compared without trailing slashes and ignoring case; `C:` when
/// `system_drive` is `None`. Returns `None` when no mount matches.
pub fn pick_primary_disk<S: AsRef<str>>(
    mounts: &[S],
    family: OsFamily,
    system_drive: Option<&str>,
) -> Option<usize> {
    match family {
        OsFamily::Linux | OsFamily::Macos => mounts.iter().position(|m| m.as_ref() == "/"),
        OsFamily::Windows => {
            let want = system_drive
                .unwrap_or("C:")
                .trim_end_matches(['\\', '/'])
                .to_ascii_lowercase();
            mounts.iter().position(|m| {
                m.as_ref()
                    .trim_end_matches(['\\', '/'])
                    .to_ascii_lowercase()
                    == want
            })
        }
    }
}

/// Mounts that are not real user-visible volumes: pseudo file systems,
/// snap/squash images, container overlays, and macOS system sub-volumes.
pub fn is_hidden_mount(mount: &str, fs: &str) -> bool {
    const PSEUDO_FS: &[&str] = &[
        "tmpfs", "devtmpfs", "devfs", "overlay", "squashfs", "autofs", "proc", "sysfs", "cgroup",
        "cgroup2", "efivarfs", "nullfs", "ramfs",
    ];
    let fs = fs.to_ascii_lowercase();
    if PSEUDO_FS.contains(&fs.as_str()) {
        return true;
    }
    if mount.starts_with("/snap/") || mount.starts_with("/var/lib/docker/") {
        return true;
    }
    // macOS: every /System/Volumes/* sub-volume (Data, VM, Preboot, ...)
    // shares the APFS container that `/` already reports, so listing them
    // would count the same space twice.
    mount.starts_with("/System/Volumes/") || mount.starts_with("/Library/Developer/CoreSimulator/")
}

/// OS identity for this host.
pub fn os_info() -> Result<OsInfo> {
    let family = OsFamily::current()
        .ok_or_else(|| ProbeError::Unsupported(format!("os info on {}", std::env::consts::OS)))?;
    let (name, version) = name_version(family);
    let arch = System::cpu_arch()
        .filter(|a| !a.trim().is_empty())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    Ok(OsInfo {
        name,
        version,
        family,
        arch: normalize_arch(&arch),
        kernel: System::kernel_version().filter(|s| !s.trim().is_empty()),
        hostname: System::host_name().filter(|s| !s.trim().is_empty()),
    })
}

#[cfg(target_os = "linux")]
fn name_version(_family: OsFamily) -> (String, String) {
    let text = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    let fields = parse_os_release(&text);
    if fields.is_empty() {
        return (
            System::name().unwrap_or_else(|| "Linux".to_string()),
            System::os_version().unwrap_or_default(),
        );
    }
    linux_name_version(&fields)
}

#[cfg(target_os = "macos")]
fn name_version(_family: OsFamily) -> (String, String) {
    // sysinfo reads kern.osproductversion, e.g. "15.1".
    (
        "macOS".to_string(),
        System::os_version().unwrap_or_default(),
    )
}

#[cfg(target_os = "windows")]
fn name_version(_family: OsFamily) -> (String, String) {
    // long_os_version is the registry ProductName with the edition,
    // e.g. "Windows 11 Pro"; os_version is e.g. "11 (26100)".
    (
        System::long_os_version().unwrap_or_else(|| "Windows".to_string()),
        System::os_version().unwrap_or_default(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn name_version(_family: OsFamily) -> (String, String) {
    // Unreachable: os_info returns Unsupported before calling this.
    (std::env::consts::OS.to_string(), String::new())
}

/// The Windows system drive (`SystemDrive`, e.g. `C:`). `None` elsewhere.
pub fn system_drive() -> Option<String> {
    if cfg!(target_os = "windows") {
        std::env::var("SystemDrive").ok().filter(|s| !s.is_empty())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = include_str!("../fixtures/os-release-ubuntu-24.04");
    const DEBIAN: &str = include_str!("../fixtures/os-release-debian-12");
    const ALPINE: &str = include_str!("../fixtures/os-release-alpine");

    #[test]
    fn ubuntu_os_release() {
        let f = parse_os_release(UBUNTU);
        assert_eq!(
            linux_name_version(&f),
            ("Ubuntu 24.04.1 LTS".to_string(), "24.04".to_string())
        );
        assert_eq!(f["ID"], "ubuntu");
        assert_eq!(f["HOME_URL"], "https://www.ubuntu.com/");
    }

    #[test]
    fn debian_os_release() {
        let f = parse_os_release(DEBIAN);
        assert_eq!(
            linux_name_version(&f),
            (
                "Debian GNU/Linux 12 (bookworm)".to_string(),
                "12".to_string()
            )
        );
    }

    #[test]
    fn alpine_bare_values_and_comments() {
        let f = parse_os_release(ALPINE);
        assert_eq!(f["ID"], "alpine");
        assert_eq!(f.len(), 6, "the comment line adds no key");
        assert_eq!(
            linux_name_version(&f),
            ("Alpine Linux v3.20".to_string(), "3.20.3".to_string())
        );
    }

    #[test]
    fn quoting_and_escapes() {
        let f = parse_os_release(
            "A=\"say \\\"hi\\\" \\$HOME\"\nB='single $x'\nC=bare\nnot a line\n=x\n",
        );
        assert_eq!(f["A"], "say \"hi\" $HOME");
        assert_eq!(f["B"], "single $x");
        assert_eq!(f["C"], "bare");
        assert_eq!(f.len(), 3);
    }

    #[test]
    fn missing_os_release_fields_fall_back() {
        let f = parse_os_release("NAME=\"Arch Linux\"\nVERSION_ID=\nVERSION=rolling\n");
        assert_eq!(
            linux_name_version(&f),
            ("Arch Linux".to_string(), "rolling".to_string())
        );
        assert_eq!(
            linux_name_version(&HashMap::new()),
            ("Linux".to_string(), String::new())
        );
    }

    #[test]
    fn arch_names() {
        assert_eq!(normalize_arch("aarch64"), "arm64");
        assert_eq!(normalize_arch("ARM64"), "arm64");
        assert_eq!(normalize_arch("amd64"), "x86_64");
        assert_eq!(normalize_arch("x86_64"), "x86_64");
        assert_eq!(normalize_arch("riscv64"), "riscv64");
    }

    #[test]
    fn primary_disk_unix_is_root() {
        let mounts = ["/boot/efi", "/", "/home"];
        assert_eq!(pick_primary_disk(&mounts, OsFamily::Linux, None), Some(1));
        let mac = ["/System/Volumes/Data", "/", "/Volumes/Backup"];
        assert_eq!(pick_primary_disk(&mac, OsFamily::Macos, None), Some(1));
        assert_eq!(pick_primary_disk(&["/data"], OsFamily::Linux, None), None);
    }

    #[test]
    fn primary_disk_windows_is_system_drive() {
        let mounts = ["D:\\", "c:\\", "E:\\"];
        assert_eq!(
            pick_primary_disk(&mounts, OsFamily::Windows, Some("C:")),
            Some(1)
        );
        assert_eq!(pick_primary_disk(&mounts, OsFamily::Windows, None), Some(1));
        assert_eq!(
            pick_primary_disk(&mounts, OsFamily::Windows, Some("D:\\")),
            Some(0)
        );
        assert_eq!(
            pick_primary_disk(&mounts, OsFamily::Windows, Some("Z:")),
            None
        );
    }

    #[test]
    fn hidden_mounts() {
        assert!(is_hidden_mount("/run", "tmpfs"));
        assert!(is_hidden_mount("/snap/core22/1663", "squashfs"));
        assert!(is_hidden_mount("/System/Volumes/VM", "apfs"));
        assert!(is_hidden_mount("/System/Volumes/Preboot", "apfs"));
        assert!(is_hidden_mount("/System/Volumes/Data", "apfs"));
        assert!(!is_hidden_mount("/", "ext4"));
        assert!(!is_hidden_mount("/Volumes/Backup", "apfs"));
        assert!(!is_hidden_mount("C:\\", "NTFS"));
    }
}
