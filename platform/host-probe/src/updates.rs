//! Pending OS and package updates (`GET /v1/host/updates`).
//!
//! Read-only: nothing here refreshes package metadata or installs anything,
//! so the answer is as fresh as the machine's own last refresh. On Debian and
//! Ubuntu [`UpdatesReport::last_refreshed`] says when that was, and `stale`
//! is set when it was more than a week ago.
//!
//! | OS | Source | Security flag |
//! |---|---|---|
//! | Debian, Ubuntu | `apt list --upgradable` | the `-security` suite in the line |
//! | Fedora, RHEL | `dnf check-update --quiet` (exit 100 = updates) | `dnf updateinfo list --security`, which also yields CVE ids |
//! | macOS | `softwareupdate --list` (slow, 20 s) | "security" in the label or title |
//! | Windows | Windows Update Agent COM through PowerShell | MSRC severity or the Security Updates category; CVE ids from the update |
//!
//! Parsers are pure and run on every OS. A missing or slow tool lands in
//! [`UpdatesReport::sources`], not in an error.
//!
//! JSON follows the contract (`id`, `name`, `current`, `available`,
//! `security`, `kind`) plus additive `source`, `cves`, `requiresRestart` and
//! `recommended`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cmd::{first_line, run, SourceStatus};
use crate::{OsFamily, ProbeError, Result};

/// `package` for a single package, `os` for a system or OS-level update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateKind {
    Package,
    Os,
}

/// Which tool reported the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSource {
    Apt,
    Dnf,
    Softwareupdate,
    Windows,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One pending update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUpdate {
    /// Stable id: package name, softwareupdate label, `KB…` number.
    pub id: String,
    pub name: String,
    /// Installed version, when the tool says (apt does; dnf, macOS and
    /// Windows do not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// Version to install; the KB number or title on Windows, which has no
    /// version string.
    pub available: String,
    pub security: bool,
    pub kind: UpdateKind,
    pub source: UpdateSource,
    /// CVE ids, only when the tool's data carries them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cves: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_restart: bool,
    /// macOS marks some updates as recommended.
    #[serde(default, skip_serializing_if = "is_false")]
    pub recommended: bool,
}

impl HostUpdate {
    fn new(source: UpdateSource, id: &str, name: &str, available: &str, kind: UpdateKind) -> Self {
        HostUpdate {
            id: id.to_string(),
            name: name.to_string(),
            current: None,
            available: available.to_string(),
            security: false,
            kind,
            source,
            cves: Vec::new(),
            requires_restart: false,
            recommended: false,
        }
    }
}

/// Updates plus the status of each source tried and apt's metadata age.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesReport {
    pub updates: Vec<HostUpdate>,
    pub sources: Vec<SourceStatus>,
    /// ISO 8601 UTC time apt last refreshed its package lists, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refreshed: Option<String>,
    /// Package lists are older than a week, so the list may miss updates.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stale: bool,
}

/// Per-tool timeouts.
#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub apt: Duration,
    pub dnf: Duration,
    /// `softwareupdate --list` talks to Apple and can be slow.
    pub softwareupdate: Duration,
    /// The Windows Update search is the slowest of the four.
    pub windows: Duration,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        UpdateOptions {
            apt: Duration::from_secs(15),
            dnf: Duration::from_secs(45),
            softwareupdate: Duration::from_secs(20),
            windows: Duration::from_secs(60),
        }
    }
}

/// Pending updates, default options. Blocking; async callers use
/// `spawn_blocking`.
pub fn updates() -> Result<Vec<HostUpdate>> {
    Ok(updates_report(&UpdateOptions::default())?.updates)
}

/// Like [`updates`] but also says which sources could be read.
pub fn updates_report(opts: &UpdateOptions) -> Result<UpdatesReport> {
    let family = OsFamily::current()
        .ok_or_else(|| ProbeError::Unsupported(format!("updates on {}", std::env::consts::OS)))?;
    let mut report = UpdatesReport {
        updates: Vec::new(),
        sources: Vec::new(),
        last_refreshed: None,
        stale: false,
    };
    let mut take = |name: &str, r: std::result::Result<Vec<HostUpdate>, String>| match r {
        Ok(mut list) => {
            report.updates.append(&mut list);
            report.sources.push(SourceStatus::ok(name));
        }
        Err(reason) => report.sources.push(SourceStatus::unavailable(name, reason)),
    };
    match family {
        OsFamily::Linux => {
            take("apt", collect_apt(opts));
            take("dnf", collect_dnf(opts));
        }
        OsFamily::Macos => take("softwareupdate", collect_softwareupdate(opts)),
        OsFamily::Windows => take("windows-update", collect_windows(opts)),
    }
    if report.sources.iter().any(|s| s.source == "apt" && s.available) {
        if let Some(age) = apt_lists_modified() {
            report.last_refreshed = Some(DateTime::<Utc>::from(age).format("%Y-%m-%dT%H:%M:%SZ").to_string());
            report.stale = SystemTime::now().duration_since(age).map(is_stale).unwrap_or(false);
        }
    }
    // Security updates first, then by name.
    report
        .updates
        .sort_by(|a, b| b.security.cmp(&a.security).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(report)
}

/// Package lists older than a week count as stale.
fn is_stale(age: Duration) -> bool {
    age > Duration::from_secs(7 * 24 * 3600)
}

// ---------------------------------------------------------------------------
// apt (Debian, Ubuntu)
// ---------------------------------------------------------------------------

/// When apt last refreshed its lists: the periodic success stamp, else the
/// lists directory.
fn apt_lists_modified() -> Option<SystemTime> {
    ["/var/lib/apt/periodic/update-success-stamp", "/var/lib/apt/lists"]
        .iter()
        .find_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

fn collect_apt(opts: &UpdateOptions) -> std::result::Result<Vec<HostUpdate>, String> {
    let o = run("apt", &["list", "--upgradable"], opts.apt).map_err(|e| e.reason("apt"))?;
    if o.code != Some(0) {
        return Err(format!("apt list failed: {}", first_line(&o.stderr)));
    }
    Ok(parse_apt(&o.stdout))
}

/// Parse `apt list --upgradable`.
///
/// ```text
/// Listing...
/// openssl/noble-updates,noble-security 3.0.13-0ubuntu3.4 amd64 [upgradable from: 3.0.13-0ubuntu3.2]
/// ```
///
/// The text after `[` is cut at its first `: ` so a translated "upgradable
/// from" still parses and an epoch (`1:2.43`) stays whole.
pub fn parse_apt(text: &str) -> Vec<HostUpdate> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((name, rest)) = line.split_once('/') else { continue };
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        let mut tok = rest.split_whitespace();
        let (Some(suites), Some(version)) = (tok.next(), tok.next()) else { continue };
        let current = rest
            .rfind('[')
            .and_then(|i| rest[i + 1..].trim_end_matches(']').split_once(": "))
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let mut u = HostUpdate::new(UpdateSource::Apt, name, name, version, UpdateKind::Package);
        u.current = current;
        u.security = suites.split(',').any(|s| s.trim().ends_with("-security"));
        out.push(u);
    }
    out
}

// ---------------------------------------------------------------------------
// dnf (Fedora, RHEL)
// ---------------------------------------------------------------------------

fn collect_dnf(opts: &UpdateOptions) -> std::result::Result<Vec<HostUpdate>, String> {
    let o = run("dnf", &["check-update", "--quiet"], opts.dnf).map_err(|e| e.reason("dnf"))?;
    match o.code {
        Some(0) => return Ok(Vec::new()),
        Some(100) => {}
        _ => return Err(format!("dnf check-update failed: {}", first_line(&o.stderr))),
    }
    let mut list = parse_dnf_check_update(&o.stdout);
    // Without the security list the updates are still right, only unflagged.
    if let Ok(sec) = run("dnf", &["updateinfo", "list", "--security", "--quiet"], opts.dnf) {
        if sec.code == Some(0) {
            apply_dnf_security(&mut list, &parse_dnf_updateinfo(&sec.stdout));
        }
    }
    Ok(list)
}

/// Parse `dnf check-update --quiet`: `name.arch  version  repo` rows. Long
/// names wrap, leaving the name alone on one line. Parsing stops at
/// "Obsoleting Packages".
pub fn parse_dnf_check_update(text: &str) -> Vec<HostUpdate> {
    let mut out = Vec::new();
    let mut pending: Option<&str> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("Obsoleting Packages") {
            break;
        }
        let tok: Vec<&str> = line.split_whitespace().collect();
        let (name_arch, version) = match (pending.take(), tok.len()) {
            (Some(n), 2) => (n, tok[0]),
            (None, 3) => (tok[0], tok[1]),
            (None, 1) => {
                pending = Some(tok[0]);
                continue;
            }
            _ => continue,
        };
        // `name.arch`: a row without an arch is a banner line.
        let Some((name, _arch)) = name_arch.rsplit_once('.') else { continue };
        out.push(HostUpdate::new(UpdateSource::Dnf, name, name, version, UpdateKind::Package));
    }
    out
}

/// One advisory row: package name (no version) and the advisory id.
#[derive(Debug, Clone, PartialEq)]
pub struct DnfAdvisory {
    pub package: String,
    pub id: String,
}

/// Parse `dnf updateinfo list --security`:
/// `RHSA-2024:1234 Important/Sec. openssl-3.0.7-28.el9_4.x86_64`.
pub fn parse_dnf_updateinfo(text: &str) -> Vec<DnfAdvisory> {
    let mut out = Vec::new();
    for line in text.lines() {
        let tok: Vec<&str> = line.split_whitespace().collect();
        if tok.len() != 3 || !tok[1].contains("/Sec") {
            continue;
        }
        if let Some(package) = nvra_name(tok[2]) {
            out.push(DnfAdvisory {
                package,
                id: tok[0].to_string(),
            });
        }
    }
    out
}

/// `openssl-3.0.7-28.el9_4.x86_64` to `openssl`.
fn nvra_name(nvra: &str) -> Option<String> {
    let (nvr, _arch) = nvra.rsplit_once('.')?;
    let mut parts = nvr.rsplitn(3, '-');
    let _release = parts.next()?;
    let _version = parts.next()?;
    parts.next().map(str::to_string)
}

/// Flag updates that have a security advisory; keep the CVE ids among them.
pub fn apply_dnf_security(updates: &mut [HostUpdate], advisories: &[DnfAdvisory]) {
    let mut by_pkg: HashMap<&str, Vec<&str>> = HashMap::new();
    for a in advisories {
        by_pkg.entry(a.package.as_str()).or_default().push(a.id.as_str());
    }
    for u in updates {
        if let Some(ids) = by_pkg.get(u.name.as_str()) {
            u.security = true;
            let mut seen = HashSet::new();
            u.cves = ids
                .iter()
                .filter(|i| i.starts_with("CVE-") && seen.insert(**i))
                .map(|i| i.to_string())
                .collect();
        }
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

fn collect_softwareupdate(opts: &UpdateOptions) -> std::result::Result<Vec<HostUpdate>, String> {
    let o = run("softwareupdate", &["--list"], opts.softwareupdate).map_err(|e| e.reason("softwareupdate"))?;
    if o.code != Some(0) {
        return Err(format!("softwareupdate failed: {}", first_line(&o.stderr)));
    }
    // Progress text goes to stderr on some releases, the list to stdout on all.
    Ok(parse_softwareupdate(&format!("{}\n{}", o.stdout, o.stderr)))
}

/// Parse `softwareupdate --list`.
///
/// ```text
/// * Label: macOS Sequoia 15.1-24B83
///     Title: macOS Sequoia 15.1, Version: 15.1, Size: 3000000KiB, Recommended: YES, Action: restart,
/// ```
///
/// Older releases write `   * macOS Sequoia 15.1-24B83` then
/// `\tmacOS Sequoia 15.1 (24B83), 3000000K [recommended] [restart]`; both
/// forms are read. Security updates are those with "security" in the label or
/// title; `macOS` titles are OS updates, the rest packages.
pub fn parse_softwareupdate(text: &str) -> Vec<HostUpdate> {
    let mut out: Vec<HostUpdate> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(label) = line.strip_prefix("* Label:") {
            out.push(su_entry(label.trim()));
        } else if let Some(label) = line.strip_prefix("* ").filter(|_| raw.starts_with(' ') || raw.starts_with('*')) {
            // Old format label line.
            out.push(su_entry(label.trim()));
        } else if let Some(last) = out.last_mut() {
            if line.starts_with("Title:") {
                su_details(last, line);
            } else if line.contains("[recommended]") || line.contains("[restart]") || line.ends_with('K') {
                // Old format detail line.
                last.recommended |= line.contains("[recommended]");
                last.requires_restart |= line.contains("[restart]");
            }
        }
    }
    out
}

fn su_entry(label: &str) -> HostUpdate {
    let mut u = HostUpdate::new(UpdateSource::Softwareupdate, label, label, label, su_kind(label));
    u.security = label.to_lowercase().contains("security");
    u
}

fn su_kind(name: &str) -> UpdateKind {
    if name.starts_with("macOS") {
        UpdateKind::Os
    } else {
        UpdateKind::Package
    }
}

/// `Title: Safari, Version: 17.6, Size: 150000KiB, Recommended: YES, Action: restart,`
fn su_details(u: &mut HostUpdate, line: &str) {
    let fields: Vec<(&str, &str)> = line
        .split(',')
        .filter_map(|p| p.split_once(':'))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let get = |key: &str| fields.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    // A title with a comma would split; the title is whatever precedes ", Version:".
    if let Some(i) = line.find(", Version:") {
        let title = line["Title:".len()..i].trim();
        if !title.is_empty() {
            u.name = title.to_string();
            u.kind = su_kind(title);
            u.security |= title.to_lowercase().contains("security");
        }
    }
    if let Some(v) = get("Version").filter(|v| !v.is_empty()) {
        u.available = v.to_string();
    }
    u.recommended = get("Recommended").map(|v| v.eq_ignore_ascii_case("yes")).unwrap_or(false);
    u.requires_restart = get("Action").map(|v| v.eq_ignore_ascii_case("restart")).unwrap_or(false);
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

const WINDOWS_SCRIPT: &str = r#"$ErrorActionPreference='Stop'
$s=New-Object -ComObject Microsoft.Update.Session
$r=$s.CreateUpdateSearcher().Search("IsInstalled=0")
$rows=@($r.Updates | ForEach-Object { [pscustomobject]@{Title=[string]$_.Title;KB=@($_.KBArticleIDs | ForEach-Object {[string]$_});Severity=[string]$_.MsrcSeverity;Categories=@($_.Categories | ForEach-Object {[string]$_.Name});Cves=@($_.CveIDs | ForEach-Object {[string]$_});RebootRequired=[bool]$_.RebootRequired} })
ConvertTo-Json -Compress -Depth 4 -InputObject $rows"#;

fn collect_windows(opts: &UpdateOptions) -> std::result::Result<Vec<HostUpdate>, String> {
    let o = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT],
        opts.windows,
    )
    .map_err(|e| e.reason("powershell"))?;
    if o.code != Some(0) {
        return Err(format!("Windows Update search failed: {}", first_line(&o.stderr)));
    }
    parse_windows_updates(&o.stdout)
}

/// Parse the JSON the PowerShell script prints: an array of
/// `{Title, KB[], Severity, Categories[], Cves[], RebootRequired}`, one bare
/// object, or nothing.
pub fn parse_windows_updates(text: &str) -> std::result::Result<Vec<HostUpdate>, String> {
    let text = text.trim().trim_start_matches('\u{feff}');
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let rows = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Null => Vec::new(),
        other => vec![other],
    };
    let strings = |r: &serde_json::Value, key: &str| -> Vec<String> {
        match r.get(key) {
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect(),
            Some(serde_json::Value::String(s)) if !s.is_empty() => vec![s.clone()],
            _ => Vec::new(),
        }
    };
    let mut out = Vec::new();
    for r in &rows {
        let Some(title) = r.get("Title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) else {
            continue;
        };
        let kbs = strings(r, "KB");
        let kb = kbs.first().map(|k| format!("KB{}", k.trim_start_matches("KB")));
        let severity = r.get("Severity").and_then(|s| s.as_str()).unwrap_or("");
        let categories = strings(r, "Categories");
        let mut u = HostUpdate::new(
            UpdateSource::Windows,
            kb.as_deref().unwrap_or(title),
            title,
            kb.as_deref().unwrap_or(title),
            UpdateKind::Os,
        );
        u.security = !severity.is_empty() || categories.iter().any(|c| c.eq_ignore_ascii_case("Security Updates"));
        u.cves = strings(r, "Cves");
        u.requires_restart = r.get("RebootRequired").and_then(|b| b.as_bool()).unwrap_or(false);
        out.push(u);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const APT: &str = include_str!("../fixtures/updates/apt-list-upgradable.txt");
    const DNF: &str = include_str!("../fixtures/updates/dnf-check-update.txt");
    const DNF_SEC: &str = include_str!("../fixtures/updates/dnf-updateinfo-security.txt");
    const SU_NEW: &str = include_str!("../fixtures/updates/softwareupdate-list.txt");
    const SU_NONE: &str = include_str!("../fixtures/updates/softwareupdate-none.txt");
    const SU_OLD: &str = include_str!("../fixtures/updates/softwareupdate-list-old.txt");
    const WIN: &str = include_str!("../fixtures/updates/windows-updates.json");

    fn by_id<'a>(v: &'a [HostUpdate], id: &str) -> &'a HostUpdate {
        v.iter().find(|u| u.id == id).unwrap_or_else(|| panic!("no {id} in {v:#?}"))
    }

    #[test]
    fn apt_security_suite_and_versions() {
        let v = parse_apt(APT);
        assert_eq!(v.len(), 4, "Listing and WARNING lines are not updates");
        let ssl = by_id(&v, "openssl");
        assert_eq!(ssl.available, "3.0.13-0ubuntu3.4");
        assert_eq!(ssl.current.as_deref(), Some("3.0.13-0ubuntu3.2"));
        assert!(ssl.security);
        assert_eq!(ssl.kind, UpdateKind::Package);
        assert_eq!(ssl.source, UpdateSource::Apt);
        let curl = by_id(&v, "curl");
        assert!(!curl.security, "noble-updates alone is not security");
        // Debian names the suite `bookworm-security`.
        assert!(by_id(&v, "libc6").security);
        assert_eq!(by_id(&v, "libc6").current.as_deref(), Some("2.36-9+deb12u3"));
        // Name with a plus and an epoch version.
        assert_eq!(by_id(&v, "libstdc++6").available, "1:14.2.0-4ubuntu2~24.04");
    }

    #[test]
    fn apt_translated_current_version_still_parses() {
        let v = parse_apt("git/noble-updates 1:2.43.0-1ubuntu7.2 amd64 [aktualisierbar von: 1:2.43.0-1ubuntu7.1]\n");
        assert_eq!(v[0].current.as_deref(), Some("1:2.43.0-1ubuntu7.1"));
        // No bracket at all: still an update, current unknown.
        let v = parse_apt("git/noble-updates 1:2.43.0-1ubuntu7.2 amd64\n");
        assert_eq!(v[0].current, None);
    }

    #[test]
    fn dnf_rows_wrapped_names_and_obsoleting() {
        let v = parse_dnf_check_update(DNF);
        let names: Vec<&str> = v.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["kernel", "openssl", "NetworkManager-wifi-and-a-very-long-package-name", "bash"]);
        assert_eq!(by_id(&v, "openssl").available, "1:3.0.7-28.el9_4");
        assert_eq!(by_id(&v, "NetworkManager-wifi-and-a-very-long-package-name").available, "1:1.46.0-26.el9_4");
        assert!(v.iter().all(|u| !u.security && u.current.is_none()));
    }

    #[test]
    fn dnf_updateinfo_marks_security_and_cves() {
        let adv = parse_dnf_updateinfo(DNF_SEC);
        assert_eq!(adv.len(), 4, "non-security and banner lines are skipped");
        assert_eq!(adv[0], DnfAdvisory { package: "openssl".into(), id: "RHSA-2024:1234".into() });
        assert_eq!(nvra_name("kernel-core-5.14.0-503.14.1.el9_5.x86_64").as_deref(), Some("kernel-core"));
        assert_eq!(nvra_name("garbage"), None);
        let mut v = parse_dnf_check_update(DNF);
        apply_dnf_security(&mut v, &adv);
        let ssl = by_id(&v, "openssl");
        assert!(ssl.security);
        assert_eq!(ssl.cves, ["CVE-2024-0727", "CVE-2024-5535"]);
        let k = by_id(&v, "kernel");
        assert!(k.security);
        assert!(k.cves.is_empty(), "an RHSA id alone is not a CVE");
        assert!(!by_id(&v, "bash").security);
    }

    #[test]
    fn softwareupdate_new_format() {
        let v = parse_softwareupdate(SU_NEW);
        assert_eq!(v.len(), 3);
        let os = by_id(&v, "macOS Sequoia 15.1-24B83");
        assert_eq!(os.name, "macOS Sequoia 15.1");
        assert_eq!(os.available, "15.1");
        assert_eq!(os.kind, UpdateKind::Os);
        assert!(os.recommended && os.requires_restart);
        assert!(!os.security);
        let safari = by_id(&v, "Safari17.6CatalinaAuto");
        assert_eq!(safari.name, "Safari");
        assert_eq!(safari.kind, UpdateKind::Package);
        assert!(!safari.requires_restart);
        let rsr = by_id(&v, "macOS Sequoia 15.1.1 Security Response-24B91");
        assert!(rsr.security, "security in the label");
    }

    #[test]
    fn softwareupdate_none_and_old_format() {
        assert!(parse_softwareupdate(SU_NONE).is_empty());
        let v = parse_softwareupdate(SU_OLD);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "macOS Ventura 13.6.1-22G313");
        assert!(v[0].recommended && v[0].requires_restart);
    }

    #[test]
    fn windows_updates_json() {
        let v = parse_windows_updates(WIN).unwrap();
        assert_eq!(v.len(), 3);
        let sec = by_id(&v, "KB5034441");
        assert!(sec.security);
        assert_eq!(sec.kind, UpdateKind::Os);
        assert_eq!(sec.cves, ["CVE-2024-20666"]);
        assert!(sec.requires_restart);
        assert!(sec.name.starts_with("2024-01 Security Update"));
        // Category only, no MSRC severity.
        assert!(by_id(&v, "KB5034765").security);
        // No KB: the title is the id, and a driver is not security.
        let drv = by_id(&v, "Intel - Net - 22.250.1.2");
        assert!(!drv.security);
        assert_eq!(drv.available, "Intel - Net - 22.250.1.2");
        let one = r#"{"Title":"Defender update","KB":["2267602"],"Severity":"","Categories":["Definition Updates"],"Cves":[],"RebootRequired":false}"#;
        let v = parse_windows_updates(one).unwrap();
        assert_eq!(v[0].id, "KB2267602");
        assert!(!v[0].security);
        assert!(parse_windows_updates("").unwrap().is_empty());
        assert!(parse_windows_updates("[]").unwrap().is_empty());
        assert!(parse_windows_updates("Exception calling Search").is_err());
    }

    #[test]
    fn staleness_is_a_week() {
        assert!(!is_stale(Duration::from_secs(6 * 24 * 3600)));
        assert!(is_stale(Duration::from_secs(8 * 24 * 3600)));
    }

    #[test]
    fn json_uses_contract_field_names() {
        let mut u = HostUpdate::new(UpdateSource::Apt, "openssl", "openssl", "3.0.13", UpdateKind::Package);
        u.current = Some("3.0.11".into());
        u.security = true;
        let v = serde_json::to_value(&u).unwrap();
        for key in ["id", "name", "current", "available", "security", "kind"] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v["kind"], "package");
        assert_eq!(v["source"], "apt");
        assert!(v.get("cves").is_none() && v.get("requiresRestart").is_none() && v.get("recommended").is_none());
        let mut w = HostUpdate::new(UpdateSource::Windows, "KB1", "x", "KB1", UpdateKind::Os);
        w.requires_restart = true;
        w.cves = vec!["CVE-2024-1".into()];
        let v = serde_json::to_value(&w).unwrap();
        assert_eq!(v["requiresRestart"], true);
        assert_eq!(v["cves"][0], "CVE-2024-1");
        assert!(v.get("current").is_none());
        let back: HostUpdate = serde_json::from_value(v).unwrap();
        assert_eq!(back, w);
    }

    #[test]
    fn updates_report_never_errors_on_known_os() {
        // Real host: a tool may be missing, but a known OS never returns Err.
        // Short timeouts keep a slow tool from stalling the suite.
        let quick = UpdateOptions {
            apt: Duration::from_secs(2),
            dnf: Duration::from_secs(2),
            softwareupdate: Duration::from_secs(2),
            windows: Duration::from_secs(2),
        };
        let r = updates_report(&quick).unwrap();
        assert!(!r.sources.is_empty());
    }
}
