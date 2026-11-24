//! Read-only host collector for the Computers console.
//!
//! This crate answers `GET /v1/host/snapshot` (see
//! `surfaces/docs/dev/computers-console-contract.mdx`). The JSON it produces
//! matches the `HostSnapshot` type in allternit-ai
//! `src/lib/computers/console-types.ts`: every field the contract names is
//! present with the same camelCase name. A few extra fields (`cpu.model`,
//! `os.kernel`, `os.hostname`, `disk[].fs`, `primaryDisk`) are additive and
//! optional, so existing TypeScript readers ignore them safely.
//!
//! The one write path is [`actions`] (`POST /v1/host/actions`): a fixed
//! action list, an approval id per action, and a change record written to
//! the runtime home before anything runs ([`changes`]).
//!
//! [`findings`] (`GET /v1/host/findings`) turns what the read modules see
//! into findings (disk nearly full, failed service, security updates, ports
//! open to the network), each with a suggested fixed-list action.
//!
//! Actions that need root run through one allow-listed admin helper
//! ([`helper`]) after a one-time setup the user approves ([`elevation`]);
//! the probe itself never runs as root.
//!
//! Everything else here is read-only and needs no root. Platform code sits behind
//! `cfg(target_os = ...)`; a host this crate does not know returns
//! [`ProbeError::Unsupported`] instead of panicking.

use serde::{Deserialize, Serialize};

pub mod actions;
pub mod changes;
pub mod cmd;
pub mod elevation;
pub mod findings;
pub mod helper;
pub mod logs;
pub mod mask;
pub mod os;
pub mod ports;
pub mod services;
pub mod snapshot;
pub mod updates;

pub use actions::{
    clean_items, run_action, ActionEngine, ActionError, ActionRequest, ActionResult, ActionType,
    ApprovalVerifier, CleanItem, Executor, ServiceOp, Snippet, SystemExecutor, UndoAction,
    WellFormedApproval,
};
pub use changes::{ChangeRecord, ChangeStatus, ChangeStore};
pub use cmd::SourceStatus;
pub use elevation::{
    ApprovalMethod, ElevationError, ElevationState, ElevationStatus, HelperKind, SetupPlan,
};
pub use findings::{
    evaluate as evaluate_findings, findings, findings_report, findings_report_with, CleanEstimate,
    FindingCategory, FindingsInput, FindingsReport, HostFinding, Severity, SuggestedAction,
};
pub use helper::{HelperError, HelperRequest, HELPER_VERSION};
pub use logs::{
    stream as stream_logs, tail as tail_logs, tail_with as tail_logs_with, LogError, LogLevel,
    LogLine, LogOptions, LogStream, LogTarget,
};
pub use mask::{mask_line, MaskOptions, Masker};
pub use ports::{
    ports, ports_report, services_with_ports, Exposure, HostPort, PortsReport, Protocol,
};
pub use services::{
    services, services_report, HostService, ProviderStatus, ServiceOptions, ServiceProvider,
    ServiceState, ServicesReport,
};
pub use snapshot::{snapshot, Sampler, SNAPSHOT_INTERVAL};
pub use updates::{
    updates, updates_report, HostUpdate, UpdateKind, UpdateOptions, UpdateSource, UpdatesReport,
};

/// Errors from the probe.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// The host OS (or this feature on it) is not supported. Carries what was
    /// asked for, for example `"snapshot on freebsd"`.
    #[error("not supported on this host: {0}")]
    Unsupported(String),
    /// Reading a system file failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ProbeError>;

/// OS family, serialized as `linux` | `macos` | `windows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsFamily {
    Linux,
    Macos,
    Windows,
}

impl OsFamily {
    /// The family this binary was built for, or `None` on any other OS.
    pub fn current() -> Option<OsFamily> {
        if cfg!(target_os = "linux") {
            Some(OsFamily::Linux)
        } else if cfg!(target_os = "macos") {
            Some(OsFamily::Macos)
        } else if cfg!(target_os = "windows") {
            Some(OsFamily::Windows)
        } else {
            None
        }
    }
}

/// One point-in-time view of the host. Field names follow the contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSnapshot {
    /// ISO 8601 UTC time the sample was taken.
    pub sampled_at: String,
    pub uptime_sec: u64,
    pub os: OsInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    /// Every real mounted volume (pseudo and snapshot mounts are left out).
    pub disk: Vec<DiskInfo>,
    /// Mount point of the system volume: `/` on Linux and macOS, the system
    /// drive (for example `C:\`) on Windows. Extra field, not in the contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_disk: Option<String>,
    pub net: NetRate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    /// Display name: `Ubuntu 24.04.1 LTS`, `macOS`, `Windows 11 Pro`.
    pub name: String,
    /// Version: os-release `VERSION_ID`, macOS product version, or the
    /// Windows version with build number.
    pub version: String,
    pub family: OsFamily,
    /// Normalized CPU architecture: `x86_64`, `arm64`, or what the OS reports.
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    /// Whole-machine CPU use, 0 to 100, one decimal.
    pub percent: f64,
    /// Logical cores.
    pub cores: u32,
    /// One-minute load average (Linux and macOS only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f64>,
    /// CPU brand string. Extra field, not in the contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub mount: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    /// File system name (`ext4`, `apfs`, `NTFS`). Extra field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<String>,
}

/// Network throughput summed over every non-loopback interface.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetRate {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> HostSnapshot {
        HostSnapshot {
            sampled_at: "2026-11-20T12:00:00Z".into(),
            uptime_sec: 3600,
            os: OsInfo {
                name: "Ubuntu 24.04.1 LTS".into(),
                version: "24.04".into(),
                family: OsFamily::Linux,
                arch: "x86_64".into(),
                kernel: Some("6.8.0-48-generic".into()),
                hostname: None,
            },
            cpu: CpuInfo {
                percent: 12.5,
                cores: 4,
                load1: Some(0.42),
                model: None,
            },
            memory: MemoryInfo {
                used_bytes: 1_000,
                total_bytes: 4_000,
            },
            disk: vec![DiskInfo {
                mount: "/".into(),
                used_bytes: 10,
                total_bytes: 100,
                fs: Some("ext4".into()),
            }],
            primary_disk: Some("/".into()),
            net: NetRate {
                rx_bytes_per_sec: 1.5,
                tx_bytes_per_sec: 0.0,
            },
        }
    }

    #[test]
    fn json_uses_contract_field_names() {
        let v = serde_json::to_value(fixture()).unwrap();
        for key in [
            "sampledAt",
            "uptimeSec",
            "os",
            "cpu",
            "memory",
            "disk",
            "net",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v["os"]["family"], "linux");
        assert_eq!(v["cpu"]["percent"], 12.5);
        assert_eq!(v["cpu"]["load1"], 0.42);
        assert!(v["cpu"].get("model").is_none(), "None extras are omitted");
        assert_eq!(v["memory"]["usedBytes"], 1_000);
        assert_eq!(v["disk"][0]["totalBytes"], 100);
        assert_eq!(v["net"]["rxBytesPerSec"], 1.5);
        assert_eq!(v["primaryDisk"], "/");
    }

    #[test]
    fn json_round_trips() {
        let s = fixture();
        let back: HostSnapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn family_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&OsFamily::Macos).unwrap(),
            "\"macos\""
        );
        assert_eq!(
            serde_json::to_string(&OsFamily::Windows).unwrap(),
            "\"windows\""
        );
    }
}
