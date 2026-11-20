//! Live and recent log lines for the Computers console
//! (`GET /v1/host/logs/stream`, one [`LogLine`] per SSE `data:` line).
//!
//! Privacy: every line is masked (see [`crate::mask`]) before it leaves this
//! module, and nothing here writes a line anywhere. Retention is a cloud
//! setting (default off); this crate has no storage.
//!
//! Sources: systemd (`journalctl`), supervisord, Docker, a file under an
//! allow-listed folder, macOS unified log, and Windows event log providers.
//! No shell is used. Names are validated before they reach a command line.
//! Dropping a [`LogStream`] kills its child process and stops its threads.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::cmd::{self, RunError};
use crate::mask::{MaskOptions, Masker};
use crate::OsFamily;

/// Longest unit / container / program name accepted.
pub const MAX_NAME_LEN: usize = 255;
/// Most lines of backlog a caller can ask for.
pub const MAX_BACKLOG: usize = 5000;
/// A message longer than this (bytes) is cut.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
const FILE_POLL: Duration = Duration::from_millis(250);
const FILE_TAIL_WINDOW: u64 = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    #[error("path is outside the allowed log folders: {0}")]
    NotAllowed(String),
    #[error("not supported on this host: {0}")]
    Unsupported(String),
    #[error("log source unavailable: {0}")]
    Unavailable(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, LogError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// One log line. Matches `LogLine` in console-types.ts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    /// ISO 8601 UTC, millisecond precision.
    pub ts: String,
    pub unit: String,
    pub level: LogLevel,
    pub message: String,
}

/// What to read logs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    /// systemd unit, for example `nginx.service`. Linux.
    Systemd { unit: String },
    /// supervisord program name.
    Supervisord { program: String },
    /// Docker container name or id.
    Docker { container: String },
    /// A file under one of [`LogOptions::file_roots`]. Rotation is followed.
    File { path: PathBuf },
    /// macOS unified log for a process name.
    Macos { process: String },
    /// Windows event log provider (usually the service name).
    Windows { provider: String },
}

impl LogTarget {
    /// Short name used for `LogLine.unit`.
    pub fn unit(&self) -> String {
        match self {
            LogTarget::Systemd { unit } => unit.clone(),
            LogTarget::Supervisord { program } => program.clone(),
            LogTarget::Docker { container } => container.clone(),
            LogTarget::File { path } => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into()),
            LogTarget::Macos { process } => process.clone(),
            LogTarget::Windows { provider } => provider.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogOptions {
    /// Lines of history before following (capped at [`MAX_BACKLOG`]). Default 100.
    pub backlog: usize,
    /// Only `error` level lines. Default false.
    pub errors_only: bool,
    pub mask: MaskOptions,
    /// Folders a `File` target may live under.
    pub file_roots: Vec<PathBuf>,
}

impl Default for LogOptions {
    fn default() -> Self {
        LogOptions { backlog: 100, errors_only: false, mask: MaskOptions::default(), file_roots: default_file_roots() }
    }
}

/// `/var/log` and `/Library/Logs` on unix (plus `~/Library/Logs` on macOS),
/// `%ProgramData%` on Windows.
pub fn default_file_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(unix) {
        v.push(PathBuf::from("/var/log"));
        v.push(PathBuf::from("/Library/Logs"));
        if cfg!(target_os = "macos") {
            if let Some(h) = std::env::var_os("HOME") {
                v.push(PathBuf::from(h).join("Library/Logs"));
            }
        }
    } else if let Some(p) = std::env::var_os("ProgramData") {
        v.push(PathBuf::from(p));
    }
    v
}

// ---------------------------------------------------------------------------
// Validation

/// Unit, container and program names: `[A-Za-z0-9@._:-]+`, at most
/// [`MAX_NAME_LEN`], and never starting with `-` (so it cannot read as a flag).
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME_LEN
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | ':' | '-'))
}

/// Windows provider names may also hold inner spaces (`Service Control Manager`).
pub fn valid_provider(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME_LEN
        && s == s.trim()
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | ':' | '-' | ' '))
}

fn check_name(kind: &str, s: &str) -> Result<()> {
    if valid_name(s) {
        Ok(())
    } else {
        Err(LogError::InvalidTarget(format!("{kind} name must match [A-Za-z0-9@._:-]+ (max {MAX_NAME_LEN}), not start with '-'")))
    }
}

/// Resolve `path` and require it to be a regular file inside one of `roots`.
pub fn resolve_file(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    let real = fs::canonicalize(path).map_err(|e| LogError::InvalidTarget(format!("{}: {e}", path.display())))?;
    let allowed = roots
        .iter()
        .filter_map(|r| fs::canonicalize(r).ok())
        .any(|r| real.starts_with(&r));
    if !allowed {
        return Err(LogError::NotAllowed(real.display().to_string()));
    }
    if !real.is_file() {
        return Err(LogError::InvalidTarget(format!("{} is not a regular file", real.display())));
    }
    Ok(real)
}

fn os_check(target: &LogTarget) -> Result<()> {
    let fam = OsFamily::current().ok_or_else(|| LogError::Unsupported("logs on this OS".into()))?;
    let (need, what) = match target {
        LogTarget::Systemd { .. } => (Some(OsFamily::Linux), "systemd logs"),
        LogTarget::Macos { .. } => (Some(OsFamily::Macos), "macOS unified log"),
        LogTarget::Windows { .. } => (Some(OsFamily::Windows), "Windows event log"),
        _ => (None, ""),
    };
    match need {
        Some(n) if n != fam => Err(LogError::Unsupported(format!("{what} on {fam:?}"))),
        _ => Ok(()),
    }
}

fn validate(target: &LogTarget, opts: &LogOptions) -> Result<Option<PathBuf>> {
    os_check(target)?;
    match target {
        LogTarget::Systemd { unit } => check_name("unit", unit).map(|_| None),
        LogTarget::Supervisord { program } => check_name("program", program).map(|_| None),
        LogTarget::Docker { container } => check_name("container", container).map(|_| None),
        LogTarget::Macos { process } => check_name("process", process).map(|_| None),
        LogTarget::Windows { provider } => {
            if valid_provider(provider) {
                Ok(None)
            } else {
                Err(LogError::InvalidTarget("provider name must match [A-Za-z0-9@._: -]+".into()))
            }
        }
        LogTarget::File { path } => resolve_file(path, &opts.file_roots).map(Some),
    }
}

// ---------------------------------------------------------------------------
// Parsing

pub fn detect_level(msg: &str) -> LogLevel {
    let head: String = msg.chars().take(80).collect::<String>().to_ascii_lowercase();
    let mut level = LogLevel::Info;
    for tok in head.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()) {
        match tok {
            "error" | "err" | "fatal" | "crit" | "critical" | "panic" | "emerg" | "alert" | "severe" => {
                return LogLevel::Error
            }
            "warn" | "warning" => level = LogLevel::Warn,
            "debug" | "trace" | "dbg" if level == LogLevel::Info => level = LogLevel::Debug,
            _ => {}
        }
    }
    level
}

fn iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn now_iso() -> String {
    iso(Utc::now())
}

fn journal_level(priority: &str) -> LogLevel {
    match priority.trim().parse::<u8>() {
        Ok(0..=3) => LogLevel::Error,
        Ok(4) => LogLevel::Warn,
        Ok(7) => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

/// One `journalctl -o json` object. Lines that are not JSON or carry no
/// `MESSAGE` give `None`.
pub fn parse_journal(unit: &str, raw: &str) -> Option<LogLine> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let message = match v.get("MESSAGE")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => {
            let bytes: Vec<u8> = a.iter().filter_map(|x| x.as_u64().map(|n| n as u8)).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => return None,
    };
    let ts = v
        .get("__REALTIME_TIMESTAMP")
        .and_then(|t| t.as_str())
        .and_then(|t| t.parse::<i64>().ok())
        .and_then(|us| Utc.timestamp_micros(us).single())
        .map(iso)
        .unwrap_or_else(now_iso);
    let level = v
        .get("PRIORITY")
        .and_then(|p| p.as_str().map(str::to_string).or_else(|| p.as_u64().map(|n| n.to_string())))
        .map(|p| journal_level(&p))
        .unwrap_or_else(|| detect_level(&message));
    Some(LogLine { ts, unit: unit.to_string(), level, message })
}

/// `docker logs --timestamps` line: `<rfc3339> <message>`.
pub fn parse_docker(unit: &str, raw: &str) -> Option<LogLine> {
    if raw.trim().is_empty() {
        return None;
    }
    let (ts, message) = match raw.split_once(' ') {
        Some((first, rest)) => match DateTime::parse_from_rfc3339(first) {
            Ok(dt) => (iso(dt.with_timezone(&Utc)), rest.to_string()),
            Err(_) => (now_iso(), raw.to_string()),
        },
        None => (now_iso(), raw.to_string()),
    };
    let level = detect_level(&message);
    Some(LogLine { ts, unit: unit.to_string(), level, message })
}

/// A line with no structure (supervisord, plain files).
pub fn parse_plain(unit: &str, raw: &str) -> Option<LogLine> {
    if raw.trim().is_empty() {
        return None;
    }
    Some(LogLine { ts: now_iso(), unit: unit.to_string(), level: detect_level(raw), message: raw.to_string() })
}

/// `log stream --style ndjson` object.
pub fn parse_macos(unit: &str, raw: &str) -> Option<LogLine> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let message = v.get("eventMessage")?.as_str()?.to_string();
    let ts = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|t| DateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S%.f%z").ok())
        .map(|dt| iso(dt.with_timezone(&Utc)))
        .unwrap_or_else(now_iso);
    let level = match v.get("messageType").and_then(|m| m.as_str()) {
        Some("Error") | Some("Fault") => LogLevel::Error,
        Some("Debug") => LogLevel::Debug,
        Some(_) => LogLevel::Info,
        None => detect_level(&message),
    };
    Some(LogLine { ts, unit: unit.to_string(), level, message })
}

/// One object from the Windows event script: `{ts, level, message}`. Event
/// levels: 1 critical, 2 error, 3 warning, 4 information, 5 verbose.
pub fn parse_windows(unit: &str, raw: &str) -> Option<LogLine> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let message = v.get("message")?.as_str()?.to_string();
    let ts = v
        .get("ts")
        .and_then(|t| t.as_str())
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| iso(dt.with_timezone(&Utc)))
        .unwrap_or_else(now_iso);
    let level = match v.get("level").and_then(|l| l.as_u64()) {
        Some(1) | Some(2) => LogLevel::Error,
        Some(3) => LogLevel::Warn,
        Some(5) => LogLevel::Debug,
        _ => LogLevel::Info,
    };
    Some(LogLine { ts, unit: unit.to_string(), level, message })
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Journal,
    Docker,
    Plain,
    Macos,
    Windows,
}

/// Parse, filter, truncate and mask: the only way a line leaves this module.
struct Pipeline {
    kind: Kind,
    unit: String,
    errors_only: bool,
    masker: Masker,
}

impl Pipeline {
    fn new(kind: Kind, unit: &str, opts: &LogOptions) -> Self {
        Pipeline { kind, unit: unit.to_string(), errors_only: opts.errors_only, masker: Masker::new(opts.mask) }
    }

    fn line(&mut self, raw: &str) -> Option<LogLine> {
        let mut l = match self.kind {
            Kind::Journal => parse_journal(&self.unit, raw),
            Kind::Docker => parse_docker(&self.unit, raw),
            Kind::Plain => parse_plain(&self.unit, raw),
            Kind::Macos => parse_macos(&self.unit, raw),
            Kind::Windows => parse_windows(&self.unit, raw),
        }?;
        if self.errors_only && l.level != LogLevel::Error {
            return None;
        }
        if l.message.len() > MAX_MESSAGE_BYTES {
            let mut cut = MAX_MESSAGE_BYTES;
            while !l.message.is_char_boundary(cut) {
                cut -= 1;
            }
            l.message.truncate(cut);
            l.message.push_str(" [cut]");
        }
        l.message = self.masker.mask_line(&l.message);
        Some(l)
    }
}

// ---------------------------------------------------------------------------
// Streaming

/// A live stream of masked lines. Iterate it, or poll with
/// [`recv_timeout`](LogStream::recv_timeout). Dropping it stops the source.
pub struct LogStream {
    rx: Receiver<LogLine>,
    stop: Arc<AtomicBool>,
    child: Option<Child>,
}

impl LogStream {
    /// Wait up to `d` for the next line.
    pub fn recv_timeout(&self, d: Duration) -> std::result::Result<LogLine, RecvTimeoutError> {
        self.rx.recv_timeout(d)
    }

    pub fn try_recv(&self) -> std::result::Result<LogLine, TryRecvError> {
        self.rx.try_recv()
    }
}

impl Iterator for LogStream {
    type Item = LogLine;
    /// Blocks until a line arrives; `None` when the source has ended.
    fn next(&mut self) -> Option<LogLine> {
        self.rx.recv().ok()
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn pump<R: Read + Send + 'static>(r: R, tx: SyncSender<LogLine>, mut p: Pipeline) {
    std::thread::spawn(move || {
        for chunk in BufReader::new(r).split(b'\n') {
            let Ok(bytes) = chunk else { break };
            let raw = String::from_utf8_lossy(&bytes);
            let raw = raw.trim_end_matches('\r');
            if let Some(l) = p.line(raw) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        }
    });
}

fn spawn_command(program: &str, args: &[String], kind: Kind, unit: &str, opts: &LogOptions, merge_stderr: bool) -> Result<LogStream> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if merge_stderr { Stdio::piped() } else { Stdio::null() })
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LogError::Unavailable(format!("{program} not found"))
            } else {
                LogError::Io(e)
            }
        })?;
    let (tx, rx) = sync_channel(1024);
    if let Some(out) = child.stdout.take() {
        pump(out, tx.clone(), Pipeline::new(kind, unit, opts));
    }
    if merge_stderr {
        if let Some(err) = child.stderr.take() {
            pump(err, tx.clone(), Pipeline::new(kind, unit, opts));
        }
    }
    drop(tx);
    Ok(LogStream { rx, stop: Arc::new(AtomicBool::new(false)), child: Some(child) })
}

fn backlog_of(opts: &LogOptions) -> usize {
    opts.backlog.min(MAX_BACKLOG)
}

const WIN_STREAM_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue';\n\
$last=(Get-Date);\n\
while($true){\n\
$ev=@(Get-WinEvent -FilterHashtable @{LogName=@('Application','System');ProviderName='__NAME__';StartTime=$last} | Sort-Object TimeCreated);\n\
foreach($e in $ev){ [pscustomobject]@{ts=$e.TimeCreated.ToUniversalTime().ToString('o');level=[int]$e.Level;message=[string]$e.Message} | ConvertTo-Json -Compress };\n\
if($ev.Count -gt 0){ $last=$ev[-1].TimeCreated.AddMilliseconds(1) };\n\
[Console]::Out.Flush();\n\
Start-Sleep -Seconds 2 }";

const WIN_TAIL_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue';\n\
Get-WinEvent -FilterHashtable @{LogName=@('Application','System');ProviderName='__NAME__'} -MaxEvents __N__ | Sort-Object TimeCreated | ForEach-Object { [pscustomobject]@{ts=$_.TimeCreated.ToUniversalTime().ToString('o');level=[int]$_.Level;message=[string]$_.Message} | ConvertTo-Json -Compress }";

fn ps_args(script: String) -> Vec<String> {
    vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script.replace('\n', " ")]
}

fn mac_predicate(process: &str, errors_only: bool) -> String {
    if errors_only {
        format!("process == \"{process}\" AND (messageType == \"Error\" OR messageType == \"Fault\")")
    } else {
        format!("process == \"{process}\"")
    }
}

/// Follow a target. The first lines are the backlog (`opts.backlog`, where
/// the source supports it), then new lines as they arrive.
pub fn stream(target: LogTarget, opts: LogOptions) -> Result<LogStream> {
    let file = validate(&target, &opts)?;
    let unit = target.unit();
    let n = backlog_of(&opts);
    match &target {
        LogTarget::Systemd { unit: u } => {
            let mut a = vec!["-u".to_string(), u.clone(), "-f".into(), "-o".into(), "json".into(), "--no-pager".into(), "--lines".into(), n.to_string()];
            if opts.errors_only {
                a.extend(["-p".to_string(), "err".to_string()]);
            }
            spawn_command("journalctl", &a, Kind::Journal, &unit, &opts, false)
        }
        LogTarget::Supervisord { program } => {
            spawn_command("supervisorctl", &["tail".into(), "-f".into(), program.clone()], Kind::Plain, &unit, &opts, false)
        }
        LogTarget::Docker { container } => spawn_command(
            "docker",
            &["logs".into(), "-f".into(), "--tail".into(), n.to_string(), "--timestamps".into(), container.clone()],
            Kind::Docker,
            &unit,
            &opts,
            true,
        ),
        LogTarget::Macos { process } => spawn_command(
            "log",
            &["stream".into(), "--style".into(), "ndjson".into(), "--predicate".into(), mac_predicate(process, opts.errors_only)],
            Kind::Macos,
            &unit,
            &opts,
            false,
        ),
        LogTarget::Windows { provider } => {
            let script = WIN_STREAM_SCRIPT.replace("__NAME__", provider);
            spawn_command("powershell", &ps_args(script), Kind::Windows, &unit, &opts, false)
        }
        LogTarget::File { .. } => {
            let path = file.expect("validated file path");
            follow_file_stream(path, &unit, n, &opts)
        }
    }
}

// ---------------------------------------------------------------------------
// Files

#[cfg(unix)]
fn file_id(md: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(md.ino())
}

#[cfg(not(unix))]
fn file_id(_md: &fs::Metadata) -> Option<u64> {
    None
}

/// Last `n` lines of a file, reading at most the last 256 KiB.
pub fn tail_file_lines(file: &mut File, n: usize) -> std::io::Result<Vec<String>> {
    let len = file.metadata()?.len();
    let start = len.saturating_sub(FILE_TAIL_WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.take(FILE_TAIL_WINDOW).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // first line is probably cut in the middle
    }
    let skip = lines.len().saturating_sub(n);
    Ok(lines[skip..].iter().map(|l| l.trim_end_matches('\r').to_string()).collect())
}

fn follow_file_stream(path: PathBuf, unit: &str, backlog: usize, opts: &LogOptions) -> Result<LogStream> {
    let mut file = File::open(&path)?;
    let initial = tail_file_lines(&mut file, backlog)?;
    // The read above leaves the cursor at the end it saw; fix the follow
    // position now so lines appended before the thread starts are not lost.
    let pos0 = file.stream_position()?;
    let id0 = file.metadata().ok().and_then(|m| file_id(&m));
    let (tx, rx) = sync_channel(1024);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    let mut p = Pipeline::new(Kind::Plain, unit, opts);
    std::thread::spawn(move || {
        let mut emit = |raw: &str| -> bool {
            match p.line(raw) {
                Some(l) => tx.send(l).is_ok(),
                None => true,
            }
        };
        for l in &initial {
            if !emit(l.as_str()) {
                return;
            }
        }
        let mut pos = pos0;
        let mut id = id0;
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            if stop_t.load(Ordering::SeqCst) {
                return;
            }
            match file.read(&mut chunk) {
                Ok(n) if n > 0 => {
                    pos += n as u64;
                    buf.extend_from_slice(&chunk[..n]);
                    while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=i).collect();
                        let s = String::from_utf8_lossy(&line[..line.len() - 1]);
                        if !emit(s.trim_end_matches('\r')) {
                            return;
                        }
                    }
                    continue;
                }
                _ => {}
            }
            std::thread::sleep(FILE_POLL);
            if let Ok(md) = fs::metadata(&path) {
                let new_id = file_id(&md);
                if new_id != id || md.len() < pos {
                    if let Ok(f) = File::open(&path) {
                        if !buf.is_empty() {
                            let s = String::from_utf8_lossy(&buf).into_owned();
                            if !emit(s.trim_end_matches('\r')) {
                                return;
                            }
                            buf.clear();
                        }
                        file = f;
                        id = new_id;
                        pos = 0;
                    }
                }
            }
        }
    });
    Ok(LogStream { rx, stop, child: None })
}

// ---------------------------------------------------------------------------
// Backlog

/// Last `lines` lines of a target, masked, with default options.
pub fn tail(target: LogTarget, lines: usize) -> Result<Vec<LogLine>> {
    tail_with(target, lines, &LogOptions::default())
}

/// Like [`tail`], with explicit options (mask, errors-only, file roots).
pub fn tail_with(target: LogTarget, lines: usize, opts: &LogOptions) -> Result<Vec<LogLine>> {
    let file = validate(&target, opts)?;
    let unit = target.unit();
    let n = lines.min(MAX_BACKLOG);
    let run = |program: &str, args: Vec<String>, secs: u64| -> Result<cmd::CmdOutput> {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = cmd::run(program, &a, Duration::from_secs(secs)).map_err(|e: RunError| LogError::Unavailable(e.reason(program)))?;
        if out.code != Some(0) && out.stdout.trim().is_empty() {
            return Err(LogError::Unavailable(format!("{program}: {}", cmd::first_line(&out.stderr))));
        }
        Ok(out)
    };
    let mut out: Vec<LogLine> = Vec::new();
    match &target {
        LogTarget::Systemd { unit: u } => {
            let mut a = vec!["-u".to_string(), u.clone(), "-o".into(), "json".into(), "--no-pager".into(), "-n".into(), n.to_string()];
            if opts.errors_only {
                a.extend(["-p".to_string(), "err".to_string()]);
            }
            collect(&run("journalctl", a, 5)?.stdout, Pipeline::new(Kind::Journal, &unit, opts), &mut out);
        }
        LogTarget::Supervisord { program } => {
            let bytes = (n.max(1) * 200).min(1_000_000);
            let a = vec!["tail".to_string(), format!("-{bytes}"), program.clone()];
            collect(&run("supervisorctl", a, 5)?.stdout, Pipeline::new(Kind::Plain, &unit, opts), &mut out);
        }
        LogTarget::Docker { container } => {
            let a = vec!["logs".to_string(), "--tail".into(), n.to_string(), "--timestamps".into(), container.clone()];
            let o = run("docker", a, 5)?;
            collect(&o.stdout, Pipeline::new(Kind::Docker, &unit, opts), &mut out);
            collect(&o.stderr, Pipeline::new(Kind::Docker, &unit, opts), &mut out);
            out.sort_by(|a, b| a.ts.cmp(&b.ts));
        }
        LogTarget::Macos { process } => {
            let a = vec![
                "show".to_string(), "--style".into(), "ndjson".into(), "--last".into(), "10m".into(),
                "--predicate".into(), mac_predicate(process, opts.errors_only),
            ];
            collect(&run("log", a, 15)?.stdout, Pipeline::new(Kind::Macos, &unit, opts), &mut out);
        }
        LogTarget::Windows { provider } => {
            let script = WIN_TAIL_SCRIPT.replace("__NAME__", provider).replace("__N__", &n.max(1).to_string());
            collect(&run("powershell", ps_args(script), 15)?.stdout, Pipeline::new(Kind::Windows, &unit, opts), &mut out);
        }
        LogTarget::File { .. } => {
            let mut f = File::open(file.expect("validated file path"))?;
            let mut p = Pipeline::new(Kind::Plain, &unit, opts);
            for raw in tail_file_lines(&mut f, n)? {
                out.extend(p.line(&raw));
            }
        }
    }
    let skip = out.len().saturating_sub(n);
    Ok(out.split_off(skip))
}

fn collect(text: &str, mut p: Pipeline, out: &mut Vec<LogLine>) {
    for raw in text.lines() {
        out.extend(p.line(raw.trim_end_matches('\r')));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Instant;

    fn opts() -> LogOptions {
        LogOptions::default()
    }

    #[test]
    fn names_are_validated() {
        for ok in ["nginx.service", "user@1000.service", "my_app-1", "a:b", "AbC.9"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-f", "a b", "a;b", "a/b", "$(x)", "a`b", "a\"b", "a\nb"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        let long = "x".repeat(256);
        assert!(!valid_name(long.as_str()));
        assert!(valid_name(&"x".repeat(255)));
        assert!(valid_provider("Service Control Manager"));
        assert!(!valid_provider(" lead"));
        assert!(!valid_provider("it's"));
        assert!(!valid_provider("a;b"));
    }

    #[test]
    fn bad_target_is_refused_before_any_command() {
        let r = stream(LogTarget::Docker { container: "-rm".into() }, opts());
        assert!(matches!(r, Err(LogError::InvalidTarget(_)) | Err(LogError::Unsupported(_))));
        let r = tail(LogTarget::Supervisord { program: "a b".into() }, 5);
        assert!(matches!(r, Err(LogError::InvalidTarget(_)) | Err(LogError::Unsupported(_))));
    }

    #[test]
    fn journal_fixture_parses() {
        let text = include_str!("../fixtures/logs/journal.ndjson");
        let lines: Vec<LogLine> = text.lines().filter_map(|l| parse_journal("nginx.service", l)).collect();
        assert_eq!(lines.len(), 5, "not-json and no-MESSAGE lines are skipped");
        assert_eq!(lines[0].ts, "2026-11-20T12:00:00.123Z");
        assert_eq!(lines[0].level, LogLevel::Info);
        assert_eq!(lines[0].message, "Started nginx.");
        assert_eq!(lines[1].level, LogLevel::Error);
        assert_eq!(lines[2].level, LogLevel::Warn);
        assert_eq!(lines[3].level, LogLevel::Debug);
        assert_eq!(lines[4].message, "hi");
        assert_eq!(lines[0].unit, "nginx.service");
    }

    #[test]
    fn docker_fixture_parses() {
        let text = include_str!("../fixtures/logs/docker.txt");
        let lines: Vec<LogLine> = text.lines().filter_map(|l| parse_docker("web", l)).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].ts, "2026-11-20T12:00:00.123Z");
        assert_eq!(lines[0].message, "listening on :8080");
        assert_eq!(lines[0].level, LogLevel::Info);
        assert_eq!(lines[1].ts, "2026-11-20T12:00:01.500Z");
        assert_eq!(lines[1].level, LogLevel::Error);
        assert_eq!(lines[2].level, LogLevel::Warn);
        let plain = parse_docker("web", "no timestamp here").unwrap();
        assert_eq!(plain.message, "no timestamp here");
    }

    #[test]
    fn macos_fixture_parses() {
        let text = include_str!("../fixtures/logs/macos.ndjson");
        let lines: Vec<LogLine> = text.lines().filter_map(|l| parse_macos("cfprefsd", l)).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].ts, "2026-11-20T12:00:00.123Z");
        assert_eq!(lines[0].level, LogLevel::Error);
        assert_eq!(lines[1].level, LogLevel::Info);
        assert_eq!(lines[2].level, LogLevel::Debug);
    }

    #[test]
    fn windows_fixture_parses() {
        let text = include_str!("../fixtures/logs/windows.ndjson");
        let lines: Vec<LogLine> = text.lines().filter_map(|l| parse_windows("MyService", l)).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].ts, "2026-11-20T12:00:00.123Z");
        assert_eq!(lines[0].level, LogLevel::Error);
        assert_eq!(lines[1].level, LogLevel::Info);
        assert_eq!(lines[2].level, LogLevel::Warn);
    }

    #[test]
    fn level_detection() {
        assert_eq!(detect_level("ERROR db down"), LogLevel::Error);
        assert_eq!(detect_level("[WARN] slow"), LogLevel::Warn);
        assert_eq!(detect_level("level=debug x"), LogLevel::Debug);
        assert_eq!(detect_level("no errors found"), LogLevel::Info);
        assert_eq!(detect_level("started"), LogLevel::Info);
    }

    #[test]
    fn pipeline_masks_filters_and_cuts() {
        let mut p = Pipeline::new(Kind::Plain, "x", &opts());
        let l = p.line("ERROR login from 8.8.8.8 password=abc").unwrap();
        assert_eq!(l.message, "ERROR login from [ip] password=[secret]");
        let mut eo = LogOptions { errors_only: true, ..opts() };
        eo.mask = MaskOptions::default();
        let mut p = Pipeline::new(Kind::Plain, "x", &eo);
        assert!(p.line("all good").is_none());
        assert!(p.line("ERROR bad").is_some());
        let mut p = Pipeline::new(Kind::Plain, "x", &opts());
        let long = format!("start {}", "é".repeat(MAX_MESSAGE_BYTES));
        let l = p.line(&long).unwrap();
        assert!(l.message.ends_with(" [cut]"));
        assert!(l.message.len() <= MAX_MESSAGE_BYTES + 8);
    }

    #[test]
    fn json_matches_console_types() {
        let l = LogLine { ts: "2026-11-20T12:00:00.000Z".into(), unit: "u".into(), level: LogLevel::Warn, message: "m".into() };
        let v = serde_json::to_value(&l).unwrap();
        assert_eq!(v["level"], "warn");
        for k in ["ts", "unit", "level", "message"] {
            assert!(v.get(k).is_some());
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("host-probe-logs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn file_target_must_be_under_a_root() {
        let root = tmp_dir("roots");
        let inside = root.join("app.log");
        fs::write(&inside, "hello\n").unwrap();
        let other = tmp_dir("other");
        let outside = other.join("secret.log");
        fs::write(&outside, "x\n").unwrap();
        let roots = vec![root.clone()];
        assert!(resolve_file(&inside, &roots).is_ok());
        assert!(matches!(resolve_file(&outside, &roots), Err(LogError::NotAllowed(_))));
        // `..` cannot climb out of the root
        let sneaky = root.join("..").join(other.file_name().unwrap()).join("secret.log");
        assert!(matches!(resolve_file(&sneaky, &roots), Err(LogError::NotAllowed(_))));
        assert!(matches!(resolve_file(&root, &roots), Err(LogError::InvalidTarget(_))));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&other);
    }

    #[test]
    fn file_tail_and_follow_with_rotation() {
        let root = tmp_dir("follow");
        let path = root.join("app.log");
        fs::write(&path, "one\ntwo\nthree password=hunter2\n").unwrap();
        let o = LogOptions { backlog: 2, file_roots: vec![root.clone()], ..opts() };

        let back = tail_with(LogTarget::File { path: path.clone() }, 2, &o).unwrap();
        let msgs: Vec<&str> = back.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(msgs, ["two", "three password=[secret]"]);

        let s = stream(LogTarget::File { path: path.clone() }, o).unwrap();
        let wait = Duration::from_secs(5);
        assert_eq!(s.recv_timeout(wait).unwrap().message, "two");
        assert_eq!(s.recv_timeout(wait).unwrap().message, "three password=[secret]");

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"four\nfi").unwrap();
        f.write_all(b"ve\n").unwrap();
        assert_eq!(s.recv_timeout(wait).unwrap().message, "four");
        assert_eq!(s.recv_timeout(wait).unwrap().message, "five");

        // rotate: move the file away and start a new one
        fs::rename(&path, root.join("app.log.1")).unwrap();
        fs::write(&path, "after rotation\n").unwrap();
        assert_eq!(s.recv_timeout(wait).unwrap().message, "after rotation");

        // truncate in place
        fs::write(&path, "short\n").unwrap();
        assert_eq!(s.recv_timeout(wait).unwrap().message, "short");

        let t = Instant::now();
        drop(s);
        assert!(t.elapsed() < Duration::from_secs(2));
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn dropping_a_command_stream_kills_the_child() {
        let o = opts();
        let args = vec!["-c".to_string(), "echo first; exec sleep 30".to_string()];
        let mut s = spawn_command("sh", &args, Kind::Plain, "t", &o, false).unwrap();
        assert_eq!(s.next().unwrap().message, "first");
        let t = Instant::now();
        drop(s);
        assert!(t.elapsed() < Duration::from_secs(2), "drop must kill and reap the child");
    }

    #[cfg(unix)]
    #[test]
    fn missing_tool_is_unavailable() {
        let o = opts();
        let r = spawn_command("allternit-no-such-tool-xyz", &[], Kind::Plain, "t", &o, false);
        assert!(matches!(r, Err(LogError::Unavailable(_))));
    }
}
