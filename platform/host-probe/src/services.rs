//! `GET /v1/host/services`: every service the host runs, from every service
//! manager it has.
//!
//! Providers, each behind `cfg` and runtime detection:
//!
//! | Provider | Where | Detection |
//! |---|---|---|
//! | `systemd` | Linux | `/run/systemd/system` exists and `systemctl` runs |
//! | `supervisord` | Linux, macOS | `supervisorctl status` answers (Orgo cloud computers use it) |
//! | `docker` | any | `docker ps -a` answers (absent or permission denied = unavailable) |
//! | `launchd` | macOS | `launchctl list` |
//! | `scm` | Windows | `Get-Service` through PowerShell, `sc queryex` as fallback |
//!
//! A provider that is missing, denied or slow is reported in
//! [`ServicesReport::providers`] as unavailable with a reason; it is never an
//! error. [`services`] only fails with [`ProbeError::Unsupported`] when no
//! provider could run on this host at all.
//!
//! Every command runs with a timeout, no shell, and fixed arguments (nothing
//! user supplied reaches a command line). Parsing is pure and separate from
//! the commands, so fixtures in `fixtures/services/` test every provider.
//!
//! Ports are filled in later by the `ports` module, so they are not here.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{OsFamily, ProbeError, Result};

/// How long any one command may run.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// Normalized service state. Serialized as `running` | `stopped` | `failed` | `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Running,
    Stopped,
    Failed,
    Unknown,
}

/// Which service manager reported the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceProvider {
    Systemd,
    Supervisord,
    Docker,
    Launchd,
    /// Windows Service Control Manager.
    Scm,
}

/// One service. Contract fields: `name`, `unit`, `state`, `enabled`,
/// `description`, `pid`, `memoryBytes`. `provider`, `cpuPercent` and `since`
/// are additive extras.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostService {
    /// Display name (unit name without `.service` on systemd, label on
    /// launchd, container name on Docker).
    pub name: String,
    /// Identifier the provider acts on: full unit name, label, container id,
    /// supervisor program, service name.
    pub unit: String,
    pub state: ServiceState,
    /// Starts on its own (systemd enabled, launchd loaded, Windows start type
    /// not Disabled, supervisor program configured).
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    pub provider: ServiceProvider,
    /// CPU use of the main process, 0 to 100 per core, one decimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    /// ISO 8601 UTC time it last became active, when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

impl HostService {
    fn new(
        provider: ServiceProvider,
        name: impl Into<String>,
        unit: impl Into<String>,
        state: ServiceState,
    ) -> Self {
        HostService {
            name: name.into(),
            unit: unit.into(),
            state,
            enabled: true,
            description: None,
            pid: None,
            memory_bytes: None,
            provider,
            cpu_percent: None,
            since: None,
        }
    }
}

/// Whether a provider could be read, and why not when it could not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: ServiceProvider,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Services plus the status of each provider that was tried.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesReport {
    pub services: Vec<HostService>,
    pub providers: Vec<ProviderStatus>,
}

/// Knobs for [`services_report`].
#[derive(Debug, Clone)]
pub struct ServiceOptions {
    /// Keep `com.apple.*` and other system noise from launchd. Default off.
    pub include_apple: bool,
    /// Timeout for each command.
    pub timeout: Duration,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        ServiceOptions {
            include_apple: false,
            timeout: COMMAND_TIMEOUT,
        }
    }
}

/// All services from every available provider, default options.
///
/// Blocking; async callers use `tokio::task::spawn_blocking`.
pub fn services() -> Result<Vec<HostService>> {
    Ok(services_report(&ServiceOptions::default())?.services)
}

/// Like [`services`] but also says which providers were available.
pub fn services_report(opts: &ServiceOptions) -> Result<ServicesReport> {
    let mut services = Vec::new();
    let mut providers = Vec::new();

    let mut take = |provider: ServiceProvider, r: std::result::Result<Vec<HostService>, String>| match r {
        Ok(mut list) => {
            list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            services.append(&mut list);
            providers.push(ProviderStatus {
                provider,
                available: true,
                reason: None,
            });
        }
        Err(reason) => providers.push(ProviderStatus {
            provider,
            available: false,
            reason: Some(reason),
        }),
    };

    #[cfg(target_os = "linux")]
    take(ServiceProvider::Systemd, collect_systemd(opts));
    #[cfg(unix)]
    take(ServiceProvider::Supervisord, collect_supervisord(opts));
    take(ServiceProvider::Docker, collect_docker(opts));
    #[cfg(target_os = "macos")]
    take(ServiceProvider::Launchd, collect_launchd(opts));
    #[cfg(target_os = "windows")]
    take(ServiceProvider::Scm, collect_scm(opts));

    if !providers.iter().any(|p| p.available) && OsFamily::current().is_none() {
        return Err(ProbeError::Unsupported(format!(
            "services on {}",
            std::env::consts::OS
        )));
    }

    enrich_with_process_stats(&mut services);
    Ok(ServicesReport { services, providers })
}

// ---------------------------------------------------------------------------
// Command runner
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CmdOutput {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
enum RunError {
    NotFound,
    Timeout,
    Io(String),
}

impl RunError {
    fn reason(&self, program: &str) -> String {
        match self {
            RunError::NotFound => format!("{program} not found"),
            RunError::Timeout => format!("{program} timed out"),
            RunError::Io(e) => format!("{program}: {e}"),
        }
    }
}

/// Run `program args` with no shell and a timeout; the child is killed when
/// the timeout passes.
fn run(program: &str, args: &[&str], timeout: Duration) -> std::result::Result<CmdOutput, RunError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RunError::NotFound
            } else {
                RunError::Io(e.to_string())
            }
        })?;

    fn drain<R: Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    }
    let out_h = drain(child.stdout.take());
    let err_h = drain(child.stderr.take());

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Reader threads end when the pipes close; do not block on
                    // them if a grandchild still holds a pipe open.
                    return Err(RunError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => return Err(RunError::Io(e.to_string())),
        }
    };
    let stdout = out_h.join().unwrap_or_default();
    let stderr = err_h.join().unwrap_or_default();
    Ok(CmdOutput {
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

// ---------------------------------------------------------------------------
// systemd (Linux)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn collect_systemd(opts: &ServiceOptions) -> std::result::Result<Vec<HostService>, String> {
    if !std::path::Path::new("/run/systemd/system").exists() {
        return Err("systemd is not the init system".into());
    }
    let t = opts.timeout;
    let base = ["list-units", "--type=service", "--all", "--no-pager"];

    // JSON needs systemd 246+; older ones fail on the flag, so fall back to text.
    let mut json_args = base.to_vec();
    json_args.push("--output=json");
    let mut list = match run("systemctl", &json_args, t) {
        Ok(o) if o.code == Some(0) => parse_systemd_json(&o.stdout).ok(),
        Ok(_) => None,
        Err(e) => return Err(e.reason("systemctl")),
    };
    if list.is_none() {
        let mut text_args = base.to_vec();
        text_args.extend(["--no-legend", "--plain"]);
        let o = run("systemctl", &text_args, t).map_err(|e| e.reason("systemctl"))?;
        if o.code != Some(0) {
            return Err(format!("systemctl list-units failed: {}", first_line(&o.stderr)));
        }
        list = Some(parse_systemd_text(&o.stdout));
    }
    let mut units = list.unwrap_or_default();
    units.retain(|u| u.load != "not-found");

    // One `show` call per chunk gives PID, start time and enabled state.
    let mut props: HashMap<String, SystemdProps> = HashMap::new();
    for chunk in units.chunks(150) {
        let mut args: Vec<&str> = vec![
            "show",
            "--no-pager",
            "-p",
            "Id,MainPID,ActiveEnterTimestamp,UnitFileState",
        ];
        args.extend(chunk.iter().map(|u| u.unit.as_str()));
        if let Ok(o) = run("systemctl", &args, t) {
            props.extend(parse_systemd_show(&o.stdout));
        }
    }
    Ok(units
        .into_iter()
        .map(|u| {
            let p = props.get(&u.unit);
            systemd_to_service(u, p)
        })
        .collect())
}

/// One row of `systemctl list-units`.
#[derive(Debug, Clone, PartialEq)]
struct SystemdUnit {
    unit: String,
    load: String,
    active: String,
    sub: String,
    description: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct SystemdProps {
    main_pid: Option<u32>,
    since: Option<String>,
    unit_file_state: Option<String>,
}

fn parse_systemd_json(text: &str) -> std::result::Result<Vec<SystemdUnit>, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| e.to_string())?;
    let arr = v.as_array().ok_or("expected a JSON array")?;
    let s = |row: &serde_json::Value, k: &str| row.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok(arr
        .iter()
        .filter(|r| r.get("unit").and_then(|x| x.as_str()).is_some())
        .map(|r| SystemdUnit {
            unit: s(r, "unit"),
            load: s(r, "load"),
            active: s(r, "active"),
            sub: s(r, "sub"),
            description: s(r, "description"),
        })
        .collect())
}

/// `UNIT LOAD ACTIVE SUB DESCRIPTION...`, whitespace separated.
fn parse_systemd_text(text: &str) -> Vec<SystemdUnit> {
    text.lines()
        .filter_map(|line| {
            // `●`/`*` marks problem units on a terminal; drop it if present.
            let line = line.trim().trim_start_matches(['●', '*', '○', '×']).trim_start();
            let mut it = line.split_whitespace();
            let unit = it.next()?;
            let load = it.next()?;
            let active = it.next()?;
            let sub = it.next()?;
            if !unit.contains('.') {
                return None;
            }
            let description = it.collect::<Vec<_>>().join(" ");
            Some(SystemdUnit {
                unit: unit.into(),
                load: load.into(),
                active: active.into(),
                sub: sub.into(),
                description,
            })
        })
        .collect()
}

/// Blocks of `Key=Value` lines separated by blank lines, keyed by `Id`.
fn parse_systemd_show(text: &str) -> HashMap<String, SystemdProps> {
    let mut out = HashMap::new();
    for block in text.split("\n\n") {
        let mut id = None;
        let mut p = SystemdProps::default();
        for line in block.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k {
                "Id" => id = Some(v.to_string()),
                "MainPID" => p.main_pid = v.parse::<u32>().ok().filter(|n| *n > 0),
                "ActiveEnterTimestamp" => p.since = parse_systemd_timestamp(v),
                "UnitFileState" if !v.is_empty() => p.unit_file_state = Some(v.to_string()),
                _ => {}
            }
        }
        if let Some(id) = id {
            out.insert(id, p);
        }
    }
    out
}

/// `Wed 2026-11-18 09:12:03 UTC` to ISO 8601. Only UTC/GMT stamps convert;
/// a local zone name cannot be resolved safely, so those give `None`.
fn parse_systemd_timestamp(v: &str) -> Option<String> {
    let parts: Vec<&str> = v.split_whitespace().collect();
    if parts.len() != 4 || !matches!(parts[3], "UTC" | "GMT") {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(&format!("{} {}", parts[1], parts[2]), "%Y-%m-%d %H:%M:%S").ok()?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn systemd_state(active: &str, sub: &str) -> ServiceState {
    match (active, sub) {
        ("active", _) | ("reloading", _) => ServiceState::Running,
        ("failed", _) => ServiceState::Failed,
        ("activating", "auto-restart") => ServiceState::Failed,
        ("inactive", _) => ServiceState::Stopped,
        _ => ServiceState::Unknown,
    }
}

fn systemd_to_service(u: SystemdUnit, p: Option<&SystemdProps>) -> HostService {
    let name = u.unit.strip_suffix(".service").unwrap_or(&u.unit).to_string();
    let mut s = HostService::new(ServiceProvider::Systemd, name, u.unit, systemd_state(&u.active, &u.sub));
    if !u.description.is_empty() {
        s.description = Some(u.description);
    }
    s.enabled = false;
    if let Some(p) = p {
        s.pid = p.main_pid;
        s.since = p.since.clone();
        s.enabled = matches!(p.unit_file_state.as_deref(), Some("enabled") | Some("enabled-runtime"));
    }
    s
}

// ---------------------------------------------------------------------------
// supervisord
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn collect_supervisord(opts: &ServiceOptions) -> std::result::Result<Vec<HostService>, String> {
    let o = run("supervisorctl", &["status"], opts.timeout).map_err(|e| e.reason("supervisorctl"))?;
    // supervisorctl exits 3 when any program is not running; that is data.
    let list = parse_supervisor(&o.stdout, Utc::now());
    if list.is_empty() && o.code != Some(0) {
        let why = if o.stderr.trim().is_empty() { &o.stdout } else { &o.stderr };
        return Err(format!("supervisorctl: {}", first_line(why)));
    }
    Ok(list)
}

/// `name  STATE  pid N, uptime H:MM:SS` lines. Unknown lines (errors) are skipped.
fn parse_supervisor(text: &str, now: DateTime<Utc>) -> Vec<HostService> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let name = it.next()?;
            let st = it.next()?;
            let state = match st {
                "RUNNING" => ServiceState::Running,
                "STOPPED" | "EXITED" => ServiceState::Stopped,
                "FATAL" | "BACKOFF" => ServiceState::Failed,
                "STARTING" | "STOPPING" | "UNKNOWN" => ServiceState::Unknown,
                _ => return None,
            };
            // Everything after the state token, from the original line.
            let rest = line.trim_start()[name.len()..].trim_start()[st.len()..].trim_start();
            let mut s = HostService::new(ServiceProvider::Supervisord, name, name, state);
            if state == ServiceState::Running {
                if let Some(p) = rest.strip_prefix("pid ") {
                    s.pid = p.split(',').next().and_then(|n| n.trim().parse().ok());
                }
                if let Some((_, up)) = rest.split_once("uptime ") {
                    if let Some(secs) = parse_supervisor_uptime(up) {
                        let t = now - chrono::Duration::seconds(secs);
                        s.since = Some(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
                    }
                }
            } else if !rest.is_empty() && state == ServiceState::Failed {
                s.description = Some(rest.to_string());
            }
            Some(s)
        })
        .collect()
}

/// `0:12:30` or `1 day, 2:03:04` (also `2 days, ...`) to seconds.
fn parse_supervisor_uptime(s: &str) -> Option<i64> {
    let s = s.trim();
    let (days, hms) = match s.split_once(',') {
        Some((d, rest)) => (d.split_whitespace().next()?.parse::<i64>().ok()?, rest.trim()),
        None => (0, s),
    };
    let mut p = hms.split(':');
    let h: i64 = p.next()?.trim().parse().ok()?;
    let m: i64 = p.next()?.trim().parse().ok()?;
    let sec: i64 = p.next()?.trim().parse().ok()?;
    Some(days * 86_400 + h * 3600 + m * 60 + sec)
}

// ---------------------------------------------------------------------------
// docker
// ---------------------------------------------------------------------------

fn collect_docker(opts: &ServiceOptions) -> std::result::Result<Vec<HostService>, String> {
    let o = run("docker", &["ps", "-a", "--format", "{{json .}}"], opts.timeout)
        .map_err(|e| e.reason("docker"))?;
    if o.code != Some(0) {
        // Daemon down, permission denied, no socket: provider unavailable.
        return Err(format!("docker unavailable: {}", first_line(&o.stderr)));
    }
    Ok(parse_docker_ps(&o.stdout))
}

/// One JSON object per line from `docker ps -a --format '{{json .}}'`.
fn parse_docker_ps(text: &str) -> Vec<HostService> {
    text.lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
            let g = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
            let id = g("ID");
            // `Names` can list several (comma separated); take the first.
            let name = g("Names").split(',').next().unwrap_or("");
            if id.is_empty() && name.is_empty() {
                return None;
            }
            let state = docker_state(g("State"), g("Status"));
            let name = if name.is_empty() { id } else { name };
            let mut s = HostService::new(ServiceProvider::Docker, name, if id.is_empty() { name } else { id }, state);
            if !g("Image").is_empty() {
                s.description = Some(g("Image").to_string());
            }
            // Restart policy is not in `ps`; a container is "enabled" when it is up.
            s.enabled = state == ServiceState::Running;
            Some(s)
        })
        .collect()
}

fn docker_state(state: &str, status: &str) -> ServiceState {
    match state {
        "running" => ServiceState::Running,
        "exited" => {
            let code = status
                .strip_prefix("Exited (")
                .and_then(|r| r.split(')').next())
                .and_then(|c| c.trim().parse::<i32>().ok());
            match code {
                Some(0) | None => ServiceState::Stopped,
                Some(_) => ServiceState::Failed,
            }
        }
        "dead" | "restarting" => ServiceState::Failed,
        "created" | "paused" => ServiceState::Stopped,
        _ => ServiceState::Unknown,
    }
}

// ---------------------------------------------------------------------------
// launchd (macOS)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn collect_launchd(opts: &ServiceOptions) -> std::result::Result<Vec<HostService>, String> {
    let o = run("launchctl", &["list"], opts.timeout).map_err(|e| e.reason("launchctl"))?;
    if o.code != Some(0) {
        return Err(format!("launchctl list failed: {}", first_line(&o.stderr)));
    }
    Ok(parse_launchctl(&o.stdout, opts.include_apple))
}

/// `PID<TAB>Status<TAB>Label`, header first. `-` means not running; a
/// non-zero status of a stopped job is its last exit code (negative: signal).
fn parse_launchctl(text: &str, include_apple: bool) -> Vec<HostService> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let pid = f.next()?;
            let status = f.next()?;
            let label = f.next()?;
            if pid == "PID" {
                return None;
            }
            if !include_apple && is_launchd_noise(label) {
                return None;
            }
            let pid_n = pid.parse::<u32>().ok();
            let state = if pid_n.is_some() {
                ServiceState::Running
            } else if status.parse::<i32>().map(|c| c == 0).unwrap_or(true) {
                ServiceState::Stopped
            } else {
                ServiceState::Failed
            };
            let mut s = HostService::new(ServiceProvider::Launchd, label, label, state);
            s.pid = pid_n;
            Some(s)
        })
        .collect()
}

/// System jobs and per-app anonymous entries that are never what a user wants.
fn is_launchd_noise(label: &str) -> bool {
    label.starts_with("com.apple.")
        || label.starts_with("application.")
        || label.starts_with("UIKitApplication:")
        || label.starts_with("0x")
}

// ---------------------------------------------------------------------------
// Windows Service Control Manager
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn collect_scm(opts: &ServiceOptions) -> std::result::Result<Vec<HostService>, String> {
    // Fixed script; nothing user supplied is interpolated.
    const SCRIPT: &str = "Get-Service | Select-Object Name,DisplayName,\
        @{n='Status';e={\"$($_.Status)\"}},@{n='StartType';e={\"$($_.StartType)\"}} | ConvertTo-Json -Compress";
    let ps = run("powershell", &["-NoProfile", "-NonInteractive", "-Command", SCRIPT], opts.timeout);
    if let Ok(o) = &ps {
        if o.code == Some(0) {
            if let Ok(list) = parse_get_service(&o.stdout) {
                return Ok(list);
            }
        }
    }
    let o = run("sc", &["queryex", "type=", "service", "state=", "all"], opts.timeout)
        .map_err(|e| e.reason("sc"))?;
    if o.code != Some(0) {
        return Err(format!("sc queryex failed: {}", first_line(&o.stdout)));
    }
    Ok(parse_sc_queryex(&o.stdout))
}

/// `Get-Service | ConvertTo-Json`. PowerShell emits a bare object for one
/// service and an array for many; `Status`/`StartType` are names or the enum
/// numbers, depending on how the script was written.
fn parse_get_service(text: &str) -> std::result::Result<Vec<HostService>, String> {
    let text = text.trim().trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let rows = match v {
        serde_json::Value::Array(a) => a,
        o @ serde_json::Value::Object(_) => vec![o],
        _ => return Err("expected object or array".into()),
    };
    Ok(rows
        .iter()
        .filter_map(|r| {
            let name = r.get("Name")?.as_str()?;
            let state = match scm_word(r.get("Status"), &["", "Stopped", "StartPending", "StopPending", "Running", "ContinuePending", "PausePending", "Paused"]) {
                "Running" => ServiceState::Running,
                "Stopped" | "Paused" => ServiceState::Stopped,
                _ => ServiceState::Unknown,
            };
            let start = scm_word(r.get("StartType"), &["Boot", "System", "Automatic", "Manual", "Disabled"]);
            // PowerShell numbers: 0 boot, 1 system, 2 automatic, 3 manual, 4 disabled.
            let mut s = HostService::new(ServiceProvider::Scm, name, name, state);
            s.enabled = start != "Disabled";
            if let Some(d) = r.get("DisplayName").and_then(|x| x.as_str()) {
                if !d.is_empty() && d != name {
                    s.description = Some(d.to_string());
                }
            }
            Some(s)
        })
        .collect())
}

/// A JSON value that is either an enum name or its number; maps numbers
/// through `names` (indexed by the enum value, except Status which starts at
/// 1 and so has a blank slot 0).
fn scm_word<'a>(v: Option<&'a serde_json::Value>, names: &[&'static str]) -> &'a str {
    match v {
        Some(serde_json::Value::String(s)) => s.as_str(),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .and_then(|i| names.get(i as usize).copied())
            .unwrap_or(""),
        _ => "",
    }
}

/// `sc queryex type= service state= all` text.
fn parse_sc_queryex(text: &str) -> Vec<HostService> {
    struct Cur {
        name: String,
        display: String,
        state: Option<String>,
        exit: i64,
        pid: u32,
    }
    fn finish(c: Cur, out: &mut Vec<HostService>) {
        let state = match c.state.as_deref() {
            Some("RUNNING") => ServiceState::Running,
            // 1077: never started since boot; any other non-zero exit code is a failure.
            Some("STOPPED") if c.exit != 0 && c.exit != 1077 => ServiceState::Failed,
            Some("STOPPED") | Some("PAUSED") => ServiceState::Stopped,
            _ => ServiceState::Unknown,
        };
        let mut s = HostService::new(ServiceProvider::Scm, c.name.clone(), c.name.clone(), state);
        if !c.display.is_empty() && c.display != c.name {
            s.description = Some(c.display);
        }
        if c.pid > 0 {
            s.pid = Some(c.pid);
        }
        out.push(s);
    }

    let mut out = Vec::new();
    let mut cur: Option<Cur> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = t.strip_prefix("SERVICE_NAME:") {
            if let Some(c) = cur.take() {
                finish(c, &mut out);
            }
            cur = Some(Cur { name: n.trim().into(), display: String::new(), state: None, exit: 0, pid: 0 });
        } else if let Some(c) = cur.as_mut() {
            if let Some(d) = t.strip_prefix("DISPLAY_NAME:") {
                c.display = d.trim().into();
            } else if let Some(v) = t.strip_prefix("STATE") {
                // `: 4  RUNNING`
                c.state = v.split_whitespace().nth(2).map(str::to_string);
            } else if let Some(v) = t.strip_prefix("WIN32_EXIT_CODE") {
                c.exit = v.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            } else if let Some(v) = t.strip_prefix("PID") {
                c.pid = v.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            }
        }
    }
    if let Some(c) = cur.take() {
        finish(c, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// Resource figures
// ---------------------------------------------------------------------------

/// Fill `memory_bytes` and `cpu_percent` from sysinfo for services that have
/// a PID. CPU needs two readings, so this waits one minimum interval, and only
/// when there is something to measure.
fn enrich_with_process_stats(services: &mut [HostService]) {
    if !services.iter().any(|s| s.pid.is_some()) {
        return;
    }
    let mut sys = sysinfo::System::new();
    sys.refresh_processes();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_processes();
    for s in services.iter_mut() {
        let Some(pid) = s.pid else { continue };
        if let Some(p) = sys.process(sysinfo::Pid::from_u32(pid)) {
            s.memory_bytes = Some(p.memory());
            s.cpu_percent = Some((f64::from(p.cpu_usage()) * 10.0).round() / 10.0);
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEMD_JSON: &str = include_str!("../fixtures/services/systemd-list-units.json");
    const SYSTEMD_TXT: &str = include_str!("../fixtures/services/systemd-list-units.txt");
    const SYSTEMD_SHOW: &str = include_str!("../fixtures/services/systemd-show.txt");
    const SUPERVISOR: &str = include_str!("../fixtures/services/supervisorctl-status.txt");
    const DOCKER: &str = include_str!("../fixtures/services/docker-ps.jsonl");
    const LAUNCHCTL: &str = include_str!("../fixtures/services/launchctl-list.txt");
    const WIN_JSON: &str = include_str!("../fixtures/services/windows-get-service.json");
    const WIN_ONE: &str = include_str!("../fixtures/services/windows-get-service-single.json");
    const WIN_SC: &str = include_str!("../fixtures/services/windows-sc-queryex.txt");

    fn by_name<'a>(v: &'a [HostService], n: &str) -> &'a HostService {
        v.iter().find(|s| s.name == n).unwrap_or_else(|| panic!("no service {n}"))
    }

    #[test]
    fn systemd_json_and_text_agree() {
        let j = parse_systemd_json(SYSTEMD_JSON).unwrap();
        let t = parse_systemd_text(SYSTEMD_TXT);
        assert_eq!(j.len(), 8);
        assert_eq!(j, t, "JSON and plain text parse to the same rows");
        assert_eq!(j[1].unit, "cron.service");
        assert_eq!(j[1].description, "Regular background program processing daemon");
    }

    #[test]
    fn systemd_json_rejects_non_json() {
        // Old systemd prints an error instead of JSON: caller falls back to text.
        assert!(parse_systemd_json("systemctl: unrecognized option '--output=json'").is_err());
    }

    #[test]
    fn systemd_state_mapping() {
        assert_eq!(systemd_state("active", "running"), ServiceState::Running);
        assert_eq!(systemd_state("active", "exited"), ServiceState::Running);
        assert_eq!(systemd_state("failed", "failed"), ServiceState::Failed);
        assert_eq!(systemd_state("activating", "auto-restart"), ServiceState::Failed);
        assert_eq!(systemd_state("activating", "start"), ServiceState::Unknown);
        assert_eq!(systemd_state("inactive", "dead"), ServiceState::Stopped);
    }

    #[test]
    fn systemd_show_gives_pid_since_enabled() {
        let props = parse_systemd_show(SYSTEMD_SHOW);
        assert_eq!(props.len(), 7);
        let cron = &props["cron.service"];
        assert_eq!(cron.main_pid, Some(612));
        assert_eq!(cron.since.as_deref(), Some("2026-11-18T09:12:03Z"));
        assert_eq!(cron.unit_file_state.as_deref(), Some("enabled"));
        assert_eq!(props["apparmor.service"].main_pid, None, "MainPID=0 is no pid");
        assert_eq!(props["nginx.service"].since, None);
        assert_eq!(props["ssh.service"].since, None, "non-UTC stamps are not guessed");
    }

    #[test]
    fn systemd_merges_into_host_services() {
        let mut units = parse_systemd_json(SYSTEMD_JSON).unwrap();
        units.retain(|u| u.load != "not-found");
        let props = parse_systemd_show(SYSTEMD_SHOW);
        let list: Vec<HostService> = units
            .into_iter()
            .map(|u| {
                let p = props.get(&u.unit).cloned();
                systemd_to_service(u, p.as_ref())
            })
            .collect();
        assert_eq!(list.len(), 7, "not-found units are dropped");
        let cron = by_name(&list, "cron");
        assert_eq!(cron.unit, "cron.service");
        assert_eq!(cron.state, ServiceState::Running);
        assert_eq!(cron.pid, Some(612));
        assert!(cron.enabled);
        assert_eq!(cron.provider, ServiceProvider::Systemd);
        let nginx = by_name(&list, "nginx");
        assert_eq!(nginx.state, ServiceState::Failed);
        assert!(!nginx.enabled);
        assert!(by_name(&list, "ssh").enabled, "enabled-runtime counts as enabled");
        assert!(!by_name(&list, "postgresql@16-main").enabled, "static is not enabled");
        assert_eq!(by_name(&list, "allternit-agent").state, ServiceState::Failed);
    }

    #[test]
    fn supervisor_status_lines() {
        let now = DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z").unwrap().with_timezone(&Utc);
        let v = parse_supervisor(SUPERVISOR, now);
        assert_eq!(v.len(), 7);
        let api = by_name(&v, "allternit-api");
        assert_eq!(api.state, ServiceState::Running);
        assert_eq!(api.pid, Some(142));
        // 1 day, 2:03:04 before noon on the 20th.
        assert_eq!(api.since.as_deref(), Some("2026-11-19T09:56:56Z"));
        assert_eq!(by_name(&v, "browser:chrome").pid, Some(188));
        assert_eq!(by_name(&v, "novnc").state, ServiceState::Stopped);
        assert_eq!(by_name(&v, "vnc").state, ServiceState::Failed);
        assert_eq!(by_name(&v, "cleanup").state, ServiceState::Stopped);
        assert_eq!(by_name(&v, "worker").state, ServiceState::Failed);
        assert_eq!(by_name(&v, "starter").state, ServiceState::Unknown);
        assert!(v.iter().all(|s| s.provider == ServiceProvider::Supervisord));
    }

    #[test]
    fn supervisor_skips_error_text() {
        let now = Utc::now();
        assert!(parse_supervisor("unix:///var/run/supervisor.sock no such file", now).is_empty());
        assert!(parse_supervisor("error: <class 'ConnectionRefusedError'>, [Errno 111]", now).is_empty());
    }

    #[test]
    fn supervisor_uptime_forms() {
        assert_eq!(parse_supervisor_uptime("0:12:30"), Some(750));
        assert_eq!(parse_supervisor_uptime("1 day, 2:03:04"), Some(93_784));
        assert_eq!(parse_supervisor_uptime("3 days, 0:00:01"), Some(259_201));
        assert_eq!(parse_supervisor_uptime("soon"), None);
    }

    #[test]
    fn docker_ps_lines() {
        let v = parse_docker_ps(DOCKER);
        assert_eq!(v.len(), 4);
        let web = by_name(&v, "web");
        assert_eq!(web.unit, "3f4a1b2c9d8e");
        assert_eq!(web.state, ServiceState::Running);
        assert_eq!(web.description.as_deref(), Some("nginx:1.27"));
        assert_eq!(by_name(&v, "db").state, ServiceState::Failed, "exit code 1");
        assert_eq!(by_name(&v, "oneshot").state, ServiceState::Stopped, "exit code 0");
        assert_eq!(by_name(&v, "cache").state, ServiceState::Failed, "restart loop");
        assert!(v.iter().all(|s| s.provider == ServiceProvider::Docker));
    }

    #[test]
    fn docker_ignores_garbage() {
        assert!(parse_docker_ps("Cannot connect to the Docker daemon").is_empty());
        assert!(parse_docker_ps("").is_empty());
    }

    #[test]
    fn launchctl_hides_apple_by_default() {
        let v = parse_launchctl(LAUNCHCTL, false);
        let names: Vec<&str> = v.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "com.allternit.desktop.helper",
                "homebrew.mxcl.postgresql@16",
                "com.docker.helper",
                "com.example.crashy"
            ]
        );
        assert_eq!(v[0].state, ServiceState::Running);
        assert_eq!(v[0].pid, Some(412));
        assert_eq!(v[1].state, ServiceState::Failed, "exit status 78");
        assert_eq!(v[2].state, ServiceState::Stopped);
        assert_eq!(v[3].state, ServiceState::Failed, "killed by signal 9");
    }

    #[test]
    fn launchctl_flag_keeps_apple() {
        let v = parse_launchctl(LAUNCHCTL, true);
        assert_eq!(v.len(), 8);
        assert_eq!(by_name(&v, "com.apple.Finder").state, ServiceState::Running);
    }

    #[test]
    fn get_service_array_and_numbers() {
        let v = parse_get_service(WIN_JSON).unwrap();
        assert_eq!(v.len(), 4);
        let sp = by_name(&v, "Spooler");
        assert_eq!(sp.state, ServiceState::Running);
        assert!(sp.enabled);
        assert_eq!(sp.description.as_deref(), Some("Print Spooler"));
        assert_eq!(by_name(&v, "W32Time").state, ServiceState::Stopped);
        assert!(!by_name(&v, "RemoteRegistry").enabled, "Disabled start type");
        let dhcp = by_name(&v, "Dhcp");
        assert_eq!(dhcp.state, ServiceState::Running, "enum number 4 = Running");
        assert!(dhcp.enabled, "enum number 2 = Automatic");
    }

    #[test]
    fn get_service_single_object_with_bom() {
        let v = parse_get_service(WIN_ONE).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Spooler");
        assert!(parse_get_service("not json").is_err());
    }

    #[test]
    fn sc_queryex_fallback() {
        let v = parse_sc_queryex(WIN_SC);
        assert_eq!(v.len(), 3);
        let sp = by_name(&v, "Spooler");
        assert_eq!(sp.state, ServiceState::Running);
        assert_eq!(sp.pid, Some(1234));
        assert_eq!(sp.description.as_deref(), Some("Print Spooler"));
        assert_eq!(by_name(&v, "W32Time").state, ServiceState::Stopped, "1077 = never started");
        assert_eq!(by_name(&v, "BadSvc").state, ServiceState::Failed);
        assert_eq!(by_name(&v, "BadSvc").pid, None);
    }

    #[test]
    fn json_uses_contract_field_names() {
        let mut s = HostService::new(ServiceProvider::Systemd, "cron", "cron.service", ServiceState::Running);
        s.pid = Some(612);
        s.memory_bytes = Some(2048);
        s.cpu_percent = Some(0.4);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["name"], "cron");
        assert_eq!(v["unit"], "cron.service");
        assert_eq!(v["state"], "running");
        assert_eq!(v["enabled"], true);
        assert_eq!(v["pid"], 612);
        assert_eq!(v["memoryBytes"], 2048);
        assert_eq!(v["provider"], "systemd");
        assert_eq!(v["cpuPercent"], 0.4);
        assert!(v.get("description").is_none() && v.get("since").is_none());
        let back: HostService = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn missing_program_is_not_found() {
        let r = run("allternit-no-such-program-xyz", &[], Duration::from_secs(1));
        assert!(matches!(r, Err(RunError::NotFound)));
    }

    #[cfg(unix)]
    #[test]
    fn slow_command_times_out() {
        let t = Instant::now();
        let r = run("sleep", &["5"], Duration::from_millis(150));
        assert!(matches!(r, Err(RunError::Timeout)));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn runner_captures_output_without_a_shell() {
        let o = run("echo", &["a;b $HOME"], Duration::from_secs(2)).unwrap();
        assert_eq!(o.code, Some(0));
        assert_eq!(o.stdout.trim(), "a;b $HOME", "arguments are not interpreted");
    }

    #[test]
    fn services_report_never_errors_on_known_os() {
        // Real host: providers may be missing, but a known OS never returns Err.
        let r = services_report(&ServiceOptions::default()).unwrap();
        assert!(!r.providers.is_empty());
    }
}
