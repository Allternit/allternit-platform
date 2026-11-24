//! Findings: `GET /v1/host/findings` → `{ findings: HostFinding[] }`.
//!
//! Plain rules over what the read modules already collect. Each finding has
//! a stable `id` (same problem, same id, across reads), a severity, and when
//! there is a fix, a `suggestedAction` `{ type, target }` from the fixed
//! action list, so a client can send it through the approval flow
//! (`POST /api/v1/computers/:id/approvals`, then `POST .../host/actions`).
//!
//! | Rule | Id | Severity | Suggested action |
//! |---|---|---|---|
//! | Disk nearly full | `disk-full:<mount>` | ≥ 90 % warning, ≥ 95 % critical | `space.clean` with the clean-up item that frees the most on that disk |
//! | More clean-up on a full disk | `space-clean:<item id>` | info | `space.clean <item id>` |
//! | Failed service | `service-failed:<unit>` | warning | `service.restart <unit>` |
//! | Security updates waiting | `security-updates` | warning (critical when one names a CVE) | `updates.install` with the one id, or `all-security` |
//! | Port open to the network | `port-exposed:<proto>:<port>` | warning | `port.free <port>` only when a known service holds it |
//!
//! [`evaluate`] is the pure rule set (tested with fixtures). [`findings`] and
//! [`findings_report`] collect the inputs on this host. Blocking; async
//! callers use `spawn_blocking`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::actions::{clean_items, ActionType};
use crate::cmd::{run, SourceStatus};
use crate::ports::{HostPort, Protocol};
use crate::services::{HostService, ServiceState};
use crate::updates::HostUpdate;
use crate::{DiskInfo, HostSnapshot, OsFamily, ProbeError, Result};

/// A disk at or above this share used is a warning.
pub const DISK_WARN_PERCENT: f64 = 90.0;
/// A disk at or above this share used is critical.
pub const DISK_CRITICAL_PERCENT: f64 = 95.0;
/// Clean-up items that free less than this are not suggested.
pub const MIN_RECLAIM_BYTES: u64 = 50 * 1024 * 1024;

const ESTIMATE_TIMEOUT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------
// Wire types (console-types.ts `HostFinding`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingCategory {
    Disk,
    Security,
    Service,
    Update,
    Config,
}

/// `HostAction` without an approval id: the client asks the cloud for one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedAction {
    #[serde(rename = "type")]
    pub action_type: ActionType,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFinding {
    /// Stable across reads: the same problem keeps the same id.
    pub id: String,
    pub severity: Severity,
    pub category: FindingCategory,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<SuggestedAction>,
}

/// Body of `GET /v1/host/findings`. `sources` is additive: which inputs could
/// be read (a missing input means fewer findings, never a made-up one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingsReport {
    pub findings: Vec<HostFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceStatus>,
}

/// How much one clean-up item would free, and on which volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanEstimate {
    /// An id from [`clean_items`].
    pub id: String,
    pub bytes: u64,
    /// The folder the item frees space in (picks the disk); `None` = system disk.
    pub path: Option<String>,
}

/// Everything the rules look at.
#[derive(Debug, Clone, Default)]
pub struct FindingsInput {
    pub disks: Vec<DiskInfo>,
    /// System volume mount (`HostSnapshot::primary_disk`).
    pub primary_disk: Option<String>,
    pub services: Vec<HostService>,
    pub ports: Vec<HostPort>,
    pub updates: Vec<HostUpdate>,
    pub clean: Vec<CleanEstimate>,
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// The rule set. Pure: same input, same findings, most severe first.
pub fn evaluate(input: &FindingsInput, os: OsFamily) -> Vec<HostFinding> {
    let mut out = Vec::new();
    disk_rules(input, os, &mut out);
    service_rules(input, &mut out);
    update_rules(input, &mut out);
    port_rules(input, &mut out);
    // Stable sort keeps rule order inside one severity.
    out.sort_by(|a, b| b.severity.cmp(&a.severity));
    out
}

fn percent(d: &DiskInfo) -> f64 {
    if d.total_bytes == 0 {
        0.0
    } else {
        d.used_bytes as f64 * 100.0 / d.total_bytes as f64
    }
}

/// Human size: `812 MB`, `1.2 GB` (powers of 1024, like the console meters).
pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i <= 2 {
        format!("{} {}", v.round() as u64, UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn same_mount(a: &str, b: &str, os: OsFamily) -> bool {
    if os == OsFamily::Windows {
        a.trim_end_matches('\\')
            .eq_ignore_ascii_case(b.trim_end_matches('\\'))
    } else {
        a == b
    }
}

/// The disk a path lives on: the longest mount that prefixes it.
fn mount_for<'a>(path: &str, disks: &'a [DiskInfo], os: OsFamily) -> Option<&'a DiskInfo> {
    let norm = |s: &str| {
        if os == OsFamily::Windows {
            s.to_ascii_lowercase().replace('/', "\\")
        } else {
            s.to_string()
        }
    };
    let p = norm(path);
    disks
        .iter()
        .filter(|d| {
            let m = norm(&d.mount);
            let m = m.trim_end_matches(['/', '\\']);
            m.is_empty()
                || p == m
                || p.starts_with(&format!("{m}/"))
                || p.starts_with(&format!("{m}\\"))
        })
        .max_by_key(|d| d.mount.len())
}

fn disk_rules(input: &FindingsInput, os: OsFamily, out: &mut Vec<HostFinding>) {
    let known: Vec<&str> = clean_items()
        .iter()
        .filter(|i| i.os.map_or(true, |o| o == os))
        .map(|i| i.id)
        .collect();
    for d in &input.disks {
        let pct = percent(d);
        if pct < DISK_WARN_PERCENT {
            continue;
        }
        let severity = if pct >= DISK_CRITICAL_PERCENT {
            Severity::Critical
        } else {
            Severity::Warning
        };
        // Clean-up items that free space on this disk, biggest first.
        let mut items: Vec<&CleanEstimate> = input
            .clean
            .iter()
            .filter(|c| known.contains(&c.id.as_str()) && c.bytes >= MIN_RECLAIM_BYTES)
            .filter(|c| {
                let on = match &c.path {
                    Some(p) => mount_for(p, &input.disks, os).map(|m| m.mount.as_str()),
                    None => input.primary_disk.as_deref(),
                };
                on.is_some_and(|m| same_mount(m, &d.mount, os))
            })
            .collect();
        items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));

        let free = d.total_bytes.saturating_sub(d.used_bytes);
        let mut detail = format!(
            "{} of {} used, {} free.",
            human_bytes(d.used_bytes),
            human_bytes(d.total_bytes),
            human_bytes(free)
        );
        if items.is_empty() {
            detail.push_str(" No clean-up item on this computer would free much here; look for large files or move data off this disk.");
        } else {
            let list: Vec<String> = items
                .iter()
                .map(|c| format!("{} (about {})", label_of(&c.id), human_bytes(c.bytes)))
                .collect();
            detail.push_str(&format!(
                " Clean-up that can free space: {}.",
                list.join("; ")
            ));
        }
        out.push(HostFinding {
            id: format!("disk-full:{}", d.mount),
            severity,
            category: FindingCategory::Disk,
            title: format!("Disk {} is {:.0}% full", d.mount, pct.floor()),
            detail,
            suggested_action: items.first().map(|c| SuggestedAction {
                action_type: ActionType::SpaceClean,
                target: c.id.clone(),
            }),
        });
        for c in items.iter().skip(1) {
            let id = format!("space-clean:{}", c.id);
            if out.iter().any(|f| f.id == id) {
                continue;
            }
            out.push(HostFinding {
                id,
                severity: Severity::Info,
                category: FindingCategory::Disk,
                title: format!("Free about {} on {}", human_bytes(c.bytes), d.mount),
                detail: format!("{}.", label_of(&c.id)),
                suggested_action: Some(SuggestedAction {
                    action_type: ActionType::SpaceClean,
                    target: c.id.clone(),
                }),
            });
        }
    }
}

fn label_of(id: &str) -> &'static str {
    clean_items()
        .iter()
        .find(|i| i.id == id)
        .map(|i| i.label)
        .unwrap_or("Clean-up")
}

fn service_rules(input: &FindingsInput, out: &mut Vec<HostFinding>) {
    let mut seen = Vec::new();
    for s in &input.services {
        if s.state != ServiceState::Failed || seen.contains(&s.unit.as_str()) {
            continue;
        }
        seen.push(s.unit.as_str());
        let mut detail = match &s.description {
            Some(d) if !d.trim().is_empty() => {
                format!("{} ({}) is in a failed state.", s.name, d.trim())
            }
            _ => format!("{} is in a failed state.", s.name),
        };
        detail.push_str(" Restarting it may bring it back; its logs say why it stopped.");
        out.push(HostFinding {
            id: format!("service-failed:{}", s.unit),
            severity: Severity::Warning,
            category: FindingCategory::Service,
            title: format!("{} has failed", s.name),
            detail,
            suggested_action: Some(SuggestedAction {
                action_type: ActionType::ServiceRestart,
                target: s.unit.clone(),
            }),
        });
    }
}

fn update_rules(input: &FindingsInput, out: &mut Vec<HostFinding>) {
    let mut sec: Vec<&HostUpdate> = input.updates.iter().filter(|u| u.security).collect();
    if sec.is_empty() {
        return;
    }
    sec.sort_by(|a, b| a.id.cmp(&b.id));
    sec.dedup_by(|a, b| a.id == b.id);
    let with_cve = sec.iter().any(|u| !u.cves.is_empty());
    let restart = sec.iter().any(|u| u.requires_restart);
    const SHOWN: usize = 10;
    let mut ids: Vec<String> = sec
        .iter()
        .take(SHOWN)
        .map(|u| {
            if u.name.is_empty() || u.name == u.id {
                u.id.clone()
            } else {
                format!("{} ({})", u.id, u.name)
            }
        })
        .collect();
    if sec.len() > SHOWN {
        ids.push(format!("and {} more", sec.len() - SHOWN));
    }
    let mut detail = format!("Waiting: {}.", ids.join(", "));
    if with_cve {
        let mut cves: Vec<&str> = sec
            .iter()
            .flat_map(|u| u.cves.iter().map(String::as_str))
            .collect();
        cves.sort_unstable();
        cves.dedup();
        let shown: Vec<&str> = cves.iter().copied().take(5).collect();
        detail.push_str(&format!(
            " Fixes {}{}.",
            shown.join(", "),
            if cves.len() > 5 { " and more" } else { "" }
        ));
    }
    if restart {
        detail.push_str(" Some need a restart afterwards.");
    }
    let n = sec.len();
    out.push(HostFinding {
        id: "security-updates".into(),
        severity: if with_cve {
            Severity::Critical
        } else {
            Severity::Warning
        },
        category: FindingCategory::Security,
        title: if n == 1 {
            "1 security update is waiting".into()
        } else {
            format!("{n} security updates are waiting")
        },
        detail,
        suggested_action: Some(SuggestedAction {
            action_type: ActionType::UpdatesInstall,
            target: if n == 1 {
                sec[0].id.clone()
            } else {
                "all-security".into()
            },
        }),
    });
}

/// Programs that normally listen on every address. Compared without case
/// and without a trailing `.exe`.
const PORT_ALLOW: &[&str] = &[
    // Remote access and the Allternit runtime itself.
    "sshd",
    "ssh",
    "tailscaled",
    "allternit-api",
    "allternit",
    "allternit-desktop",
    "ao",
    "gizzi",
    "gizzi-code",
    // Linux network plumbing.
    "systemd-resolved",
    "systemd-network",
    "systemd-networkd",
    "networkmanager",
    "dhclient",
    "dhcpcd",
    "chronyd",
    "ntpd",
    "avahi-daemon",
    "cupsd",
    // macOS system services.
    "mdnsresponder",
    "rapportd",
    "controlce",
    "controlcenter",
    "sharingd",
    "launchd",
    "airplayxpchelper",
    // Windows system services.
    "system",
    "svchost",
    "lsass",
    "wininit",
    "services",
    "spoolsv",
];

/// With no process name (it belongs to another user and we are not root),
/// only these ports are taken as the usual service.
const PORT_ALLOW_UNKNOWN: &[u16] = &[22];

fn program_key(p: &str) -> String {
    let base = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let lower = base.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn is_wildcard(addr: &str) -> bool {
    matches!(
        addr.trim_matches(|c| c == '[' || c == ']'),
        "0.0.0.0" | "::" | "*" | "::0" | "0:0:0:0:0:0:0:0"
    )
}

fn port_rules(input: &FindingsInput, out: &mut Vec<HostFinding>) {
    // One finding per (protocol, port); 0.0.0.0 and :: rows merge.
    let mut by_port: BTreeMap<(u16, &'static str), Vec<&HostPort>> = BTreeMap::new();
    for p in &input.ports {
        // UDP wildcard listeners are almost all discovery daemons (mDNS,
        // DHCP, NTP); the rule looks at TCP only.
        if p.protocol != Protocol::Tcp || p.mesh || !is_wildcard(&p.address) {
            continue;
        }
        let allowed = match &p.process {
            Some(name) => PORT_ALLOW.contains(&program_key(name).as_str()),
            None => PORT_ALLOW_UNKNOWN.contains(&p.port),
        };
        if !allowed {
            by_port.entry((p.port, "tcp")).or_default().push(p);
        }
    }
    for ((port, proto), rows) in by_port {
        let process = rows.iter().find_map(|r| r.process.clone());
        let pid = rows.iter().find_map(|r| r.pid);
        let service = input
            .services
            .iter()
            .find(|s| (pid.is_some() && s.pid == pid) || s.ports.contains(&port));
        let who = match (&process, pid) {
            (Some(p), Some(id)) => format!("{p} (pid {id})"),
            (Some(p), None) => p.clone(),
            (None, Some(id)) => format!("pid {id}"),
            (None, None) => "a program this account cannot see".into(),
        };
        let mut detail = format!(
            "{who} listens on every network address on {proto} port {port}, so other machines can reach it."
        );
        match service {
            Some(s) => detail.push_str(&format!(
                " It belongs to the service {}. If it should only be reachable from this computer, bind it to 127.0.0.1 or stop it.",
                s.name
            )),
            None => detail.push_str(
                " If it should only be reachable from this computer, bind it to 127.0.0.1 or close it in the firewall.",
            ),
        }
        out.push(HostFinding {
            id: format!("port-exposed:{proto}:{port}"),
            severity: Severity::Warning,
            category: FindingCategory::Security,
            title: match &process {
                Some(p) => format!("Port {port} is open to the network ({p})"),
                None => format!("Port {port} is open to the network"),
            },
            detail,
            suggested_action: service.map(|_| SuggestedAction {
                action_type: ActionType::PortFree,
                target: port.to_string(),
            }),
        });
    }
}

// ---------------------------------------------------------------------------
// Clean-up size estimates (read-only)
// ---------------------------------------------------------------------------

/// Bytes `docker system df` says can be reclaimed, summed over its rows.
/// Input is `docker system df --format '{{json .}}'` (one JSON object per line).
pub fn parse_docker_df(text: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut any = false;
    for line in text.lines().map(str::trim).filter(|l| l.starts_with('{')) {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        let r = v.get("Reclaimable")?.as_str()?;
        // "1.23GB (45%)" or "0B"
        let size = r.split_whitespace().next()?;
        total = total.saturating_add(parse_docker_size(size)?);
        any = true;
    }
    any.then_some(total)
}

/// Docker's sizes: decimal units (`kB`, `MB`, `GB`, `TB`) or `B`.
pub fn parse_docker_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(split);
    let n: f64 = num.trim().parse().ok()?;
    let mul = match unit.to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => return None,
    };
    (n >= 0.0).then(|| (n * mul).round() as u64)
}

/// `brew cleanup --dry-run`: "This operation would free approximately 1.2GB of disk space."
/// Homebrew uses powers of 1024 (`KB`, `MB`, `GB`).
pub fn parse_brew_dry_run(text: &str) -> Option<u64> {
    let line = text
        .lines()
        .find(|l| l.contains("would free approximately"))?;
    let rest = line.split("approximately").nth(1)?.trim();
    let size = rest.split_whitespace().next()?;
    let split = size.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = size.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mul = match unit.to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mul).round() as u64)
}

/// Total size of regular files under `dir` (no symlinks followed), with an
/// optional filter on each file. Unreadable entries are skipped.
fn dir_size(dir: &Path, keep: &dyn Fn(&Path, &std::fs::Metadata) -> bool) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    let mut entries = 0usize;
    std::fs::symlink_metadata(dir).ok().filter(|m| m.is_dir())?;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            entries += 1;
            if entries > 200_000 {
                return Some(total);
            }
            let Ok(m) = e.metadata() else { continue };
            let p = e.path();
            if m.is_dir() {
                stack.push(p);
            } else if m.is_file() && keep(&p, &m) {
                total = total.saturating_add(m.len());
            }
        }
    }
    Some(total)
}

fn estimate(id: &str, os: OsFamily) -> Option<CleanEstimate> {
    let (bytes, path): (Option<u64>, Option<String>) = match (id, os) {
        ("apt-cache", OsFamily::Linux) => {
            let dir = "/var/cache/apt/archives";
            let debs = |p: &Path, _: &std::fs::Metadata| p.extension().is_some_and(|e| e == "deb");
            (dir_size(Path::new(dir), &debs), Some(dir.into()))
        }
        ("dnf-cache", OsFamily::Linux) => {
            let dir = "/var/cache/dnf";
            let rpms = |p: &Path, _: &std::fs::Metadata| p.extension().is_some_and(|e| e == "rpm");
            (dir_size(Path::new(dir), &rpms), Some(dir.into()))
        }
        ("journal-vacuum", OsFamily::Linux) => {
            // journalctl --vacuum-time=7d removes archived files older than 7 days.
            let dir = "/var/log/journal";
            let week = Duration::from_secs(7 * 24 * 3600);
            let old = move |p: &Path, m: &std::fs::Metadata| {
                p.extension()
                    .is_some_and(|e| e == "journal" || e == "journal~")
                    && m.modified()
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > week)
            };
            (dir_size(Path::new(dir), &old), Some(dir.into()))
        }
        ("docker-prune", _) => {
            let out = run(
                "docker",
                &["system", "df", "--format", "{{json .}}"],
                ESTIMATE_TIMEOUT,
            )
            .ok()?;
            if out.code != Some(0) {
                return None;
            }
            let path = match os {
                OsFamily::Linux => Some("/var/lib/docker".to_string()),
                _ => None,
            };
            (parse_docker_df(&out.stdout), path)
        }
        ("brew-cleanup", OsFamily::Macos) => {
            let out = run(
                "brew",
                &["cleanup", "--dry-run", "--prune=all"],
                ESTIMATE_TIMEOUT,
            )
            .ok()?;
            let text = format!("{}\n{}", out.stdout, out.stderr);
            let bytes = parse_brew_dry_run(&text).or_else(|| {
                // Nothing to free prints no summary line.
                (out.code == Some(0)).then_some(0)
            });
            (
                bytes,
                dirs::home_dir().map(|h| {
                    h.join("Library/Caches/Homebrew")
                        .to_string_lossy()
                        .to_string()
                }),
            )
        }
        _ => (None, None),
    };
    Some(CleanEstimate {
        id: id.into(),
        bytes: bytes?,
        path,
    })
}

/// Estimates for every clean-up item that applies to this OS. Only called
/// when a disk is nearly full (some estimates run a command).
pub fn clean_estimates(os: OsFamily) -> Vec<CleanEstimate> {
    clean_items()
        .iter()
        .filter(|i| i.os.map_or(true, |o| o == os))
        .filter_map(|i| estimate(i.id, os))
        .collect()
}

// ---------------------------------------------------------------------------
// Collect on this host
// ---------------------------------------------------------------------------

/// Findings for this host from a snapshot the caller already has (the node
/// keeps one `Sampler`). Reads services, ports and updates; estimates
/// clean-up sizes only when a disk is nearly full.
pub fn findings_report_with(snapshot: &HostSnapshot) -> Result<FindingsReport> {
    let os = OsFamily::current()
        .ok_or_else(|| ProbeError::Unsupported(format!("findings on {}", std::env::consts::OS)))?;
    let mut sources = Vec::new();
    let mut input = FindingsInput {
        disks: snapshot.disk.clone(),
        primary_disk: snapshot.primary_disk.clone(),
        ..FindingsInput::default()
    };
    match crate::ports::services_with_ports() {
        Ok(s) => {
            input.services = s;
            sources.push(SourceStatus::ok("services"));
        }
        Err(e) => sources.push(SourceStatus::unavailable("services", e.to_string())),
    }
    match crate::ports::ports() {
        Ok(p) => {
            input.ports = p;
            sources.push(SourceStatus::ok("ports"));
        }
        Err(e) => sources.push(SourceStatus::unavailable("ports", e.to_string())),
    }
    match crate::updates::updates() {
        Ok(u) => {
            input.updates = u;
            sources.push(SourceStatus::ok("updates"));
        }
        Err(e) => sources.push(SourceStatus::unavailable("updates", e.to_string())),
    }
    if input.disks.iter().any(|d| percent(d) >= DISK_WARN_PERCENT) {
        input.clean = clean_estimates(os);
    }
    Ok(FindingsReport {
        findings: evaluate(&input, os),
        sources,
    })
}

/// [`findings_report_with`] with a fresh snapshot.
pub fn findings_report() -> Result<FindingsReport> {
    findings_report_with(&crate::snapshot::snapshot()?)
}

/// Just the findings, default options.
pub fn findings() -> Result<Vec<HostFinding>> {
    Ok(findings_report()?.findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::Exposure;
    use crate::services::ServiceProvider;
    use crate::updates::{UpdateKind, UpdateSource};

    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;

    fn disk(mount: &str, used: u64, total: u64) -> DiskInfo {
        DiskInfo {
            mount: mount.into(),
            used_bytes: used,
            total_bytes: total,
            fs: Some("ext4".into()),
        }
    }

    fn svc(unit: &str, state: ServiceState, pid: Option<u32>, ports: Vec<u16>) -> HostService {
        HostService {
            name: unit.trim_end_matches(".service").into(),
            unit: unit.into(),
            state,
            enabled: true,
            description: Some(format!("{unit} daemon")),
            pid,
            memory_bytes: None,
            provider: ServiceProvider::Systemd,
            cpu_percent: None,
            since: None,
            ports,
        }
    }

    fn port(port: u16, addr: &str, process: Option<&str>, pid: Option<u32>) -> HostPort {
        HostPort {
            port,
            protocol: Protocol::Tcp,
            address: addr.into(),
            process: process.map(Into::into),
            pid,
            exposure: if is_wildcard(addr) {
                Exposure::Public
            } else {
                Exposure::Loopback
            },
            mesh: false,
        }
    }

    fn update(id: &str, security: bool, cves: &[&str]) -> HostUpdate {
        HostUpdate {
            id: id.into(),
            name: id.into(),
            current: None,
            available: "2.0".into(),
            security,
            kind: UpdateKind::Package,
            source: UpdateSource::Apt,
            cves: cves.iter().map(|c| c.to_string()).collect(),
            requires_restart: false,
            recommended: security,
        }
    }

    /// A healthy Ubuntu server: roomy disks, services running, only sshd and
    /// loopback listeners, no security updates.
    fn healthy() -> FindingsInput {
        FindingsInput {
            disks: vec![
                disk("/", 40 * GB, 100 * GB),
                disk("/data", 890 * GB, 1000 * GB),
            ],
            primary_disk: Some("/".into()),
            services: vec![
                svc(
                    "nginx.service",
                    ServiceState::Running,
                    Some(900),
                    vec![8080],
                ),
                svc("ssh.service", ServiceState::Running, Some(700), vec![22]),
                svc("backup.service", ServiceState::Stopped, None, vec![]),
            ],
            ports: vec![
                port(22, "0.0.0.0", Some("sshd"), Some(700)),
                port(22, "::", Some("sshd"), Some(700)),
                port(8080, "127.0.0.1", Some("nginx"), Some(900)),
                port(5432, "::1", Some("postgres"), Some(1200)),
                port(41641, "100.101.102.103", Some("tailscaled"), Some(50)),
            ],
            updates: vec![update("vim", false, &[])],
            clean: vec![CleanEstimate {
                id: "apt-cache".into(),
                bytes: 2 * GB,
                path: Some("/var/cache/apt/archives".into()),
            }],
        }
    }

    #[test]
    fn healthy_host_has_no_findings() {
        let f = evaluate(&healthy(), OsFamily::Linux);
        assert!(f.is_empty(), "{f:#?}");
    }

    #[test]
    fn disk_at_90_warns_and_at_95_is_critical_with_clean_items() {
        let mut input = healthy();
        input.disks[0] = disk("/", 92 * GB, 100 * GB);
        input.clean = vec![
            CleanEstimate {
                id: "journal-vacuum".into(),
                bytes: 600 * MB,
                path: Some("/var/log/journal".into()),
            },
            CleanEstimate {
                id: "apt-cache".into(),
                bytes: 2 * GB,
                path: Some("/var/cache/apt/archives".into()),
            },
            CleanEstimate {
                id: "docker-prune".into(),
                bytes: 3 * GB,
                path: Some("/var/lib/docker".into()),
            },
            // Too small to suggest.
            CleanEstimate {
                id: "dnf-cache".into(),
                bytes: 10 * MB,
                path: Some("/var/cache/dnf".into()),
            },
            // Not a Linux item.
            CleanEstimate {
                id: "brew-cleanup".into(),
                bytes: 5 * GB,
                path: None,
            },
        ];
        let f = evaluate(&input, OsFamily::Linux);
        let d = f
            .iter()
            .find(|x| x.id == "disk-full:/")
            .expect("disk finding");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.category, FindingCategory::Disk);
        assert_eq!(d.title, "Disk / is 92% full");
        assert_eq!(
            d.suggested_action,
            Some(SuggestedAction {
                action_type: ActionType::SpaceClean,
                target: "docker-prune".into()
            })
        );
        assert!(
            d.detail.contains("about 3.0 GB")
                && d.detail.contains("about 2.0 GB")
                && d.detail.contains("about 600 MB"),
            "{}",
            d.detail
        );
        assert!(
            !d.detail.contains("Homebrew") && !d.detail.contains("dnf"),
            "{}",
            d.detail
        );
        let extra: Vec<&str> = f
            .iter()
            .filter(|x| x.id.starts_with("space-clean:"))
            .map(|x| x.id.as_str())
            .collect();
        assert_eq!(
            extra,
            ["space-clean:apt-cache", "space-clean:journal-vacuum"]
        );
        for x in f.iter().filter(|x| x.id.starts_with("space-clean:")) {
            assert_eq!(x.severity, Severity::Info);
            let a = x.suggested_action.as_ref().unwrap();
            assert_eq!(a.action_type, ActionType::SpaceClean);
            assert!(clean_items().iter().any(|i| i.id == a.target));
        }

        input.disks[0] = disk("/", 96 * GB, 100 * GB);
        let f = evaluate(&input, OsFamily::Linux);
        assert_eq!(f[0].id, "disk-full:/");
        assert_eq!(f[0].severity, Severity::Critical);
    }

    #[test]
    fn clean_items_only_attach_to_their_own_disk() {
        let mut input = healthy();
        // /data full, but every clean-up item frees space on /.
        input.disks[1] = disk("/data", 960 * GB, 1000 * GB);
        let f = evaluate(&input, OsFamily::Linux);
        assert_eq!(f.len(), 1, "{f:#?}");
        assert_eq!(f[0].id, "disk-full:/data");
        assert_eq!(f[0].severity, Severity::Critical);
        assert!(f[0].suggested_action.is_none());
        assert!(f[0].detail.contains("No clean-up item"));
    }

    #[test]
    fn windows_items_without_a_path_go_to_the_system_drive() {
        let input = FindingsInput {
            disks: vec![
                disk("C:\\", 95 * GB, 100 * GB),
                disk("D:\\", 10 * GB, 100 * GB),
            ],
            primary_disk: Some("C:\\".into()),
            clean: vec![CleanEstimate {
                id: "docker-prune".into(),
                bytes: GB,
                path: None,
            }],
            ..FindingsInput::default()
        };
        let f = evaluate(&input, OsFamily::Windows);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "disk-full:C:\\");
        assert_eq!(
            f[0].suggested_action.as_ref().unwrap().target,
            "docker-prune"
        );
    }

    #[test]
    fn failed_service_suggests_restart_by_unit() {
        let mut input = healthy();
        input
            .services
            .push(svc("worker.service", ServiceState::Failed, None, vec![]));
        let f = evaluate(&input, OsFamily::Linux);
        assert_eq!(f.len(), 1, "{f:#?}");
        assert_eq!(f[0].id, "service-failed:worker.service");
        assert_eq!(f[0].severity, Severity::Warning);
        assert_eq!(f[0].category, FindingCategory::Service);
        assert_eq!(f[0].title, "worker has failed");
        assert_eq!(
            f[0].suggested_action,
            Some(SuggestedAction {
                action_type: ActionType::ServiceRestart,
                target: "worker.service".into()
            })
        );
    }

    #[test]
    fn security_updates_name_their_ids() {
        let mut input = healthy();
        input.updates.push(update("openssl", true, &[]));
        let f = evaluate(&input, OsFamily::Linux);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "security-updates");
        assert_eq!(f[0].severity, Severity::Warning);
        assert_eq!(f[0].title, "1 security update is waiting");
        assert_eq!(f[0].suggested_action.as_ref().unwrap().target, "openssl");
        assert_eq!(
            f[0].suggested_action.as_ref().unwrap().action_type,
            ActionType::UpdatesInstall
        );

        input
            .updates
            .push(update("KB5034441", true, &["CVE-2024-20666"]));
        let f = evaluate(&input, OsFamily::Linux);
        assert_eq!(f[0].severity, Severity::Critical);
        assert_eq!(f[0].title, "2 security updates are waiting");
        assert_eq!(
            f[0].suggested_action.as_ref().unwrap().target,
            "all-security"
        );
        assert!(
            f[0].detail.contains("KB5034441") && f[0].detail.contains("openssl"),
            "{}",
            f[0].detail
        );
        assert!(f[0].detail.contains("CVE-2024-20666"));
    }

    #[test]
    fn exposed_port_warns_and_frees_only_a_known_service() {
        let mut input = healthy();
        // Unknown program on every address: warning, no action.
        input
            .ports
            .push(port(9000, "0.0.0.0", Some("python3"), Some(4242)));
        // Known service on :: and 0.0.0.0: one finding, port.free.
        input.services.push(svc(
            "redis-server.service",
            ServiceState::Running,
            Some(3333),
            vec![6379],
        ));
        input
            .ports
            .push(port(6379, "0.0.0.0", Some("redis-server"), Some(3333)));
        input
            .ports
            .push(port(6379, "::", Some("redis-server"), Some(3333)));
        // UDP and loopback and mesh rows are ignored.
        let mut udp = port(5353, "0.0.0.0", Some("python3"), Some(4242));
        udp.protocol = Protocol::Udp;
        input.ports.push(udp);
        let mut mesh = port(7000, "0.0.0.0", Some("python3"), Some(4242));
        mesh.mesh = true;
        input.ports.push(mesh);
        // Windows-style names are allow-listed without case or .exe.
        input
            .ports
            .push(port(135, "0.0.0.0", Some("svchost.exe"), Some(1000)));

        let f = evaluate(&input, OsFamily::Linux);
        let ids: Vec<&str> = f.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(
            ids,
            ["port-exposed:tcp:6379", "port-exposed:tcp:9000"],
            "{f:#?}"
        );
        let redis = &f[0];
        assert_eq!(redis.severity, Severity::Warning);
        assert_eq!(redis.category, FindingCategory::Security);
        assert_eq!(
            redis.suggested_action,
            Some(SuggestedAction {
                action_type: ActionType::PortFree,
                target: "6379".into()
            })
        );
        assert!(redis.detail.contains("redis-server"));
        assert!(f[1].suggested_action.is_none());
        assert!(f[1].title.contains("python3"));
    }

    #[test]
    fn unknown_process_on_ssh_port_is_fine_elsewhere_it_warns() {
        let mut input = healthy();
        input.ports = vec![
            port(22, "0.0.0.0", None, None),
            port(3306, "[::]", None, None),
        ];
        let f = evaluate(&input, OsFamily::Linux);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "port-exposed:tcp:3306");
        assert!(f[0].detail.contains("cannot see"));
    }

    #[test]
    fn json_matches_console_types() {
        let mut input = healthy();
        input
            .services
            .push(svc("worker.service", ServiceState::Failed, None, vec![]));
        let f = evaluate(&input, OsFamily::Linux);
        let v = serde_json::to_value(FindingsReport {
            findings: f,
            sources: vec![],
        })
        .unwrap();
        let x = &v["findings"][0];
        assert_eq!(x["id"], "service-failed:worker.service");
        assert_eq!(x["severity"], "warning");
        assert_eq!(x["category"], "service");
        assert_eq!(x["suggestedAction"]["type"], "service.restart");
        assert_eq!(x["suggestedAction"]["target"], "worker.service");
        assert!(x.get("suggested_action").is_none());
        assert!(v.get("sources").is_none());
        // Severity names are the contract's.
        assert_eq!(
            serde_json::to_value(Severity::Critical).unwrap(),
            "critical"
        );
        assert_eq!(serde_json::to_value(Severity::Info).unwrap(), "info");
    }

    #[test]
    fn ids_are_stable_across_reads() {
        let mut input = healthy();
        input.disks[0] = disk("/", 93 * GB, 100 * GB);
        input
            .services
            .push(svc("worker.service", ServiceState::Failed, None, vec![]));
        let a: Vec<String> = evaluate(&input, OsFamily::Linux)
            .into_iter()
            .map(|f| f.id)
            .collect();
        input.disks[0] = disk("/", 94 * GB, 100 * GB);
        let b: Vec<String> = evaluate(&input, OsFamily::Linux)
            .into_iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn docker_df_fixture() {
        let text = r#"{"Active":"2","Reclaimable":"1.2GB (45%)","Size":"2.6GB","TotalCount":"5","Type":"Images"}
{"Active":"1","Reclaimable":"0B (0%)","Size":"12kB","TotalCount":"1","Type":"Containers"}
{"Active":"0","Reclaimable":"300MB (100%)","Size":"300MB","TotalCount":"2","Type":"Local Volumes"}
{"Active":"0","Reclaimable":"512.5kB","Size":"512.5kB","TotalCount":"9","Type":"Build Cache"}"#;
        assert_eq!(
            parse_docker_df(text),
            Some(1_200_000_000 + 300_000_000 + 512_500)
        );
        assert_eq!(parse_docker_df(""), None);
        assert_eq!(parse_docker_df("Cannot connect to the Docker daemon"), None);
        assert_eq!(parse_docker_size("0B"), Some(0));
        assert_eq!(parse_docker_size("1.5TB"), Some(1_500_000_000_000));
        assert_eq!(parse_docker_size("12XB"), None);
    }

    #[test]
    fn brew_dry_run_fixture() {
        let text =
            "Would remove: /Users/joe/Library/Caches/Homebrew/wget--1.21.bottle.tar.gz (1.5MB)\n\
==> This operation would free approximately 1.2GB of disk space.\n";
        assert_eq!(
            parse_brew_dry_run(text),
            Some((1.2 * GB as f64).round() as u64)
        );
        assert_eq!(parse_brew_dry_run("Would remove: x (12KB)\n==> This operation would free approximately 812.4MB of disk space."), Some((812.4 * MB as f64).round() as u64));
        assert_eq!(parse_brew_dry_run(""), None);
    }

    #[test]
    fn human_bytes_reads_well() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(600 * MB), "600 MB");
        assert_eq!(human_bytes(3 * GB), "3.0 GB");
        assert_eq!(human_bytes(1536 * GB), "1.5 TB");
    }

    #[test]
    fn mount_lookup_uses_the_longest_prefix() {
        let disks = vec![
            disk("/", 1, 2),
            disk("/var", 1, 2),
            disk("/var/lib/docker", 1, 2),
        ];
        assert_eq!(
            mount_for("/var/lib/docker", &disks, OsFamily::Linux)
                .unwrap()
                .mount,
            "/var/lib/docker"
        );
        assert_eq!(
            mount_for("/var/cache/apt", &disks, OsFamily::Linux)
                .unwrap()
                .mount,
            "/var"
        );
        assert_eq!(
            mount_for("/variable", &disks, OsFamily::Linux)
                .unwrap()
                .mount,
            "/"
        );
        let win = vec![disk("C:\\", 1, 2), disk("D:\\", 1, 2)];
        assert_eq!(
            mount_for("d:/docker", &win, OsFamily::Windows)
                .unwrap()
                .mount,
            "D:\\"
        );
    }
}
