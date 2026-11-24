//! Adopt (terminal workspace, step 5): move a coding-agent session from macOS Terminal (or
//! iTerm2, Ghostty, any other app) into an Allternit terminal, open a copy of it, or open a new
//! session of the same CLI in the same folder.
//!
//! The job lives here, next to discovery ([`super::live_sessions`]) and the harness table, and
//! takes the terminal create as a function, because the create path (the pane engine socket and
//! the terminal records) lives in allternit-api, which depends on this crate. allternit-api calls
//! [`adopt`] as a library, so it works with the Factory engine's HTTP down.
//!
//! Rules (design decision 5 and amendment decision 11, one writer per session id):
//! - The request names only the session (`harness` + `sessionId`) and the terminal size. The
//!   folder, name and carried flags come from the live session itself, so a client cannot put
//!   its own argv into the command.
//! - One job per session id at a time (a process-wide lock): a second → 409 `adopt_in_progress`.
//! - Move rechecks the session by running discovery again right before it acts: exactly one
//!   holder, outside Allternit, the same pid and start time the job first saw, and idle. Busy
//!   waits (`waiting_idle`, re-polled every 2 s for up to 5 min, cancelable).
//! - Move sends SIGTERM only, to that pid only, after checking its start time once more, and
//!   waits up to 15 s for it to exit (pid reuse is caught by the start time). If it does not
//!   exit, the job fails with `did_not_exit` and nothing is started. There is no SIGKILL.
//! - Only after the old process is gone is the new terminal created with the resume command.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::harness_table as ht;
use super::live_sessions::{Holder, LiveSession};

/// How often a busy session is rechecked.
pub const WAIT_IDLE_EVERY: Duration = Duration::from_secs(2);
/// How long a Move waits for a busy session to go idle.
pub const WAIT_IDLE_FOR: Duration = Duration::from_secs(5 * 60);
/// How long a Move waits for the old process to exit after SIGTERM.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a finished job stays visible on `LiveSession.move`.
pub const KEEP_FOR: Duration = Duration::from_secs(10 * 60);

// ---------------------------------------------------------------- contract

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Move,
    Copy,
    New,
    Cancel,
}

/// `POST /api/v1/live-sessions/adopt` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptRequest {
    /// Needed for move, copy and cancel. `new` can name the session by `address` instead
    /// (a process whose session id was not found still has an address).
    #[serde(default)]
    pub session_id: String,
    pub harness: String,
    pub mode: Mode,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cols() -> u16 {
    120
}

fn default_rows() -> u16 {
    40
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveState {
    #[default]
    WaitingIdle,
    Stopping,
    Starting,
    Done,
    Failed,
}

impl MoveState {
    fn finished(self) -> bool {
        matches!(self, MoveState::Done | MoveState::Failed)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MoveState::WaitingIdle => "waiting_idle",
            MoveState::Stopping => "stopping",
            MoveState::Starting => "starting",
            MoveState::Done => "done",
            MoveState::Failed => "failed",
        }
    }
}

/// `LiveSession.move`: where a Move job is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveJob {
    pub job_id: String,
    pub state: MoveState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    /// Failed: the error code (`still_busy`, `did_not_exit`, `start_failed`, `cancelled`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Failed: what happened, in words. For `start_failed` it holds the exact resume command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// An adopt refusal: HTTP status, a stable code and a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl AdoptError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
}

impl std::fmt::Display for AdoptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// What a successful adopt call returns.
#[derive(Debug, Clone, PartialEq)]
pub enum Adopted {
    /// Move accepted: the job runs on; watch `LiveSession.move` or [`job`].
    Job(MoveJob),
    /// Copy or New: the terminal is running.
    Terminal(String),
    /// Cancel: the job as it ended.
    Cancelled(MoveJob),
}

/// What the terminal create gets: the agent command, its folder, a title and the size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSpec {
    pub argv: Vec<String>,
    pub cwd: String,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
}

pub type CreateFuture = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
/// Creates an Allternit terminal and returns its id (allternit-api's terminal create).
pub type Creator = Arc<dyn Fn(CreateSpec) -> CreateFuture + Send + Sync>;

// ---------------------------------------------------------------- the decision (pure)

/// What a Move does next, from one look at the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Idle, one external holder: stop this pid (start time checked again before the signal).
    Stop { pid: u32, started_at: String },
    /// Mid-turn: look again in [`WAIT_IDLE_EVERY`].
    WaitIdle,
}

/// Start times compare with whitespace normalised (`ps` pads one-digit days with a space).
pub fn norm_start(s: &str) -> String {
    s.split_whitespace().map(|w| if w.bytes().all(|b| b.is_ascii_digit()) { w.trim_start_matches('0').to_string() } else { w.to_string() }).collect::<Vec<_>>().join(" ")
}

/// The Move decision for one look at a session. `first`: the holder the job saw when it began
/// (`None` on that first look); a different pid or start time now means the process was
/// replaced, and the job refuses rather than stop a process it did not check.
pub fn decide_move(s: &LiveSession, first: Option<&Holder>) -> Result<Action, AdoptError> {
    if s.holders.len() > 1 {
        let apps: Vec<&str> = s.holders.iter().map(|h| h.app.as_str()).collect();
        return Err(AdoptError::new(409, "session_open_elsewhere", format!("This session is open in {} places ({}). Close all but one, then move it.", s.holders.len(), apps.join(", "))));
    }
    if s.movable == "here" || s.holders.first().is_some_and(|h| h.app == "Allternit") {
        return Err(AdoptError::new(409, "already_here", "This session already runs in an Allternit terminal."));
    }
    if s.movable == "open_elsewhere" {
        return Err(AdoptError::new(409, "session_open_elsewhere", "Another process of this CLI may hold this session. Close it, then move the session."));
    }
    if s.movable == "no_resume" {
        return Err(AdoptError::new(422, "no_resume", format!("{} can't reopen this session by id. Use Open a new session here instead.", s.harness)));
    }
    let Some(holder) = s.holders.first() else {
        return Err(AdoptError::new(409, "no_holder", "No running process holds this session, so there is nothing to move. Resume it from its CLI."));
    };
    let Some(started_at) = holder.started_at.as_deref().filter(|t| !t.trim().is_empty()) else {
        return Err(AdoptError::new(409, "holder_unverified", "The start time of this session's process could not be read, so it can't be safely stopped."));
    };
    if let Some(f) = first {
        let same_start = f.started_at.as_deref().map(norm_start) == Some(norm_start(started_at));
        if f.pid != holder.pid || !same_start {
            return Err(AdoptError::new(409, "holder_changed", "The process holding this session changed since the move began. Nothing was stopped."));
        }
    }
    match s.movable.as_str() {
        "busy" => Ok(Action::WaitIdle),
        "now" => Ok(Action::Stop { pid: holder.pid, started_at: started_at.to_string() }),
        other => Err(AdoptError::new(409, "not_movable", format!("This session can't move now ({other})."))),
    }
}

// ---------------------------------------------------------------- stop and wait

/// One look at a pid.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Probe {
    /// No such process (or a zombie waiting for its parent).
    Gone,
    /// Running, with its `ps -o lstart` start time (local time).
    Running(String),
}

#[cfg(unix)]
fn probe(pid: u32) -> Probe {
    // SAFETY: signal 0 only probes for existence.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return Probe::Gone;
    }
    let pid_s = pid.to_string();
    let Some(out) = super::proc_scan::run_with_timeout("ps", &["-o", "stat=,lstart=", "-p", &pid_s], Duration::from_secs(3)) else {
        // ps itself failed: report what kill says (alive), with no start time to match.
        return Probe::Running(String::new());
    };
    let line = out.trim();
    if line.is_empty() {
        return Probe::Gone;
    }
    let mut words = line.split_whitespace();
    let stat = words.next().unwrap_or_default();
    if stat.starts_with('Z') {
        return Probe::Gone;
    }
    Probe::Running(words.collect::<Vec<_>>().join(" "))
}

#[cfg(not(unix))]
fn probe(_pid: u32) -> Probe {
    Probe::Running(String::new())
}

/// The OS start time of a running pid as `ps -o lstart` prints it (local time).
pub fn start_time(pid: u32) -> Option<String> {
    match probe(pid) {
        Probe::Running(t) if !t.is_empty() => Some(t),
        _ => None,
    }
}

/// Whether `expected` (a holder's `startedAt`) names the process whose `ps -o lstart` is
/// `lstart`. Holders carry `ps` local time (the process scan), UTC in the same shape (Claude
/// Code's `procStart`), or RFC 3339 (a recorded start a moment after exec).
pub fn start_matches(expected: &str, lstart: &str) -> bool {
    if lstart.trim().is_empty() || expected.trim().is_empty() {
        return false;
    }
    let want = norm_start(expected);
    if want == norm_start(lstart) {
        return true;
    }
    let Ok(local) = chrono::NaiveDateTime::parse_from_str(&norm_start(lstart), "%a %b %d %H:%M:%S %Y") else { return false };
    let Some(at) = chrono::TimeZone::from_local_datetime(&chrono::Local, &local).single().map(|d| d.with_timezone(&chrono::Utc)) else { return false };
    if norm_start(&at.format("%a %b %e %H:%M:%S %Y").to_string()) == want {
        return true;
    }
    chrono::DateTime::parse_from_rfc3339(expected.trim()).is_ok_and(|t| {
        let d = t.with_timezone(&chrono::Utc).signed_duration_since(at).num_seconds();
        (-1..=10).contains(&d)
    })
}

/// SIGTERM `pid` (only after its start time matches `started_at`) and wait up to `timeout` for
/// it to exit. Never sends SIGKILL. `Ok` = the process is gone (exited, or its pid now belongs to
/// a newer process). Errors: `holder_changed` (start time differs: nothing was signalled),
/// `did_not_exit` (still running at the timeout).
pub fn stop_and_wait(pid: u32, started_at: &str, timeout: Duration) -> Result<(), AdoptError> {
    stop_and_wait_os(pid, started_at, timeout)
}

#[cfg(not(unix))]
fn stop_and_wait_os(_pid: u32, _started_at: &str, _timeout: Duration) -> Result<(), AdoptError> {
    Err(AdoptError::new(501, "unsupported", "Move is not available on this OS yet. Nothing was stopped."))
}

#[cfg(unix)]
fn stop_and_wait_os(pid: u32, started_at: &str, timeout: Duration) -> Result<(), AdoptError> {
    match probe(pid) {
        // It exited by itself after the recheck: nothing to stop.
        Probe::Gone => return Ok(()),
        Probe::Running(t) if !start_matches(started_at, &t) => {
            return Err(AdoptError::new(409, "holder_changed", format!("Process {pid} is not the one that held this session (start time {t:?}, expected {started_at:?}). Nothing was stopped.")));
        }
        Probe::Running(_) => {}
    }
    // SAFETY: plain SIGTERM to the one pid whose start time was just checked.
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if r != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(AdoptError::new(409, "stop_refused", format!("Could not signal process {pid}: {e}. Nothing was started.")));
    }
    let deadline = Instant::now() + timeout;
    loop {
        match probe(pid) {
            Probe::Gone => return Ok(()),
            // A different process got the pid: ours exited.
            Probe::Running(t) if !t.is_empty() && !start_matches(started_at, &t) => return Ok(()),
            Probe::Running(_) => {}
        }
        if Instant::now() >= deadline {
            return Err(AdoptError::new(409, "did_not_exit", format!("The session's process ({pid}) did not exit within {} s of SIGTERM. Nothing was started; quit it in its own window and try again.", timeout.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// [`stop_and_wait`], then `start` only when the old process is gone. The error says nothing
/// was started; `start` is never called on an error.
pub async fn stop_then_start<F, Fut>(pid: u32, started_at: String, timeout: Duration, start: F) -> Result<String, AdoptError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, AdoptError>>,
{
    tokio::task::spawn_blocking(move || stop_and_wait(pid, &started_at, timeout))
        .await
        .map_err(|e| AdoptError::new(500, "internal", format!("the stop task failed: {e}. Nothing was started.")))??;
    start().await
}

// ---------------------------------------------------------------- jobs and the per-session lock

type Key = (String, String);

#[derive(Default)]
struct Jobs {
    /// Sessions with an adopt call or Move job in flight.
    active: HashSet<Key>,
    /// Move jobs, with when they finished.
    moves: HashMap<Key, (MoveJob, Option<Instant>)>,
}

fn jobs() -> MutexGuard<'static, Jobs> {
    static JOBS: OnceLock<Mutex<Jobs>> = OnceLock::new();
    let mut j = JOBS.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner());
    j.moves.retain(|_, (_, done)| done.map_or(true, |t| t.elapsed() < KEEP_FOR));
    j
}

/// Held while an adopt call or Move job runs for a session; dropping it frees the session.
struct SessionLock(Option<Key>);

impl Drop for SessionLock {
    fn drop(&mut self) {
        if let Some(k) = self.0.take() {
            jobs().active.remove(&k);
        }
    }
}

fn lock(key: &Key) -> Result<SessionLock, AdoptError> {
    if key.1.is_empty() {
        return Ok(SessionLock(None));
    }
    let mut j = jobs();
    if !j.active.insert(key.clone()) {
        return Err(AdoptError::new(409, "adopt_in_progress", "This session is already being moved or opened. Wait for that to finish."));
    }
    Ok(SessionLock(Some(key.clone())))
}

/// Change a Move job that is still running. Returns false when it already ended (cancelled).
fn update(key: &Key, job_id: &str, f: impl FnOnce(&mut MoveJob)) -> bool {
    let mut j = jobs();
    match j.moves.get_mut(key) {
        Some((job, done)) if job.job_id == job_id && !job.state.finished() => {
            f(job);
            if job.state.finished() {
                *done = Some(Instant::now());
            }
            true
        }
        _ => false,
    }
}

fn fail(key: &Key, job_id: &str, e: &AdoptError) {
    update(key, job_id, |j| {
        j.state = MoveState::Failed;
        j.code = Some(e.code.to_string());
        j.error = Some(e.message.clone());
    });
}

/// The Move job for a session, while it runs and for [`KEEP_FOR`] after.
pub fn move_job(harness: &str, session_id: &str) -> Option<MoveJob> {
    jobs().moves.get(&(harness.to_string(), session_id.to_string())).map(|(j, _)| j.clone())
}

/// A Move job by its id.
pub fn job(job_id: &str) -> Option<MoveJob> {
    jobs().moves.values().find(|(j, _)| j.job_id == job_id).map(|(j, _)| j.clone())
}

/// Put each session's Move job on `LiveSession.move`.
pub fn annotate(list: &mut [LiveSession]) {
    for s in list {
        s.move_job = move_job(&s.harness, &s.session_id);
    }
}

/// [`annotate`] for a JSON session list (the engine's reply, proxied).
pub fn annotate_json(list: &mut [Value]) {
    for s in list {
        let (Some(h), Some(id)) = (s["harness"].as_str(), s["sessionId"].as_str()) else { continue };
        if let Some(j) = move_job(h, id).and_then(|j| serde_json::to_value(j).ok()) {
            s["move"] = j;
        }
    }
}

// ---------------------------------------------------------------- the job

/// The session as discovery sees it now, with its full folder.
async fn find(harness: String, session_id: String, address: Option<String>) -> Option<LiveSession> {
    tokio::task::spawn_blocking(move || {
        let all = super::live_sessions::discover_full_paths(&super::live_sessions::default_person(), None);
        if session_id.is_empty() {
            let address = address?;
            all.into_iter().find(|s| s.harness == harness && s.address.eq_ignore_ascii_case(&address))
        } else {
            all.into_iter().find(|s| s.harness == harness && s.session_id == session_id)
        }
    })
    .await
    .ok()
    .flatten()
}

/// The command the user can run by hand, shell-quoted.
pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| if !a.is_empty() && a.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:=@+,".contains(&b)) { a.clone() } else { format!("'{}'", a.replace('\'', r"'\''")) })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The holder's own executable for `argv[0]` when its argv named it bare (`claude` → the binary
/// it runs), so the new terminal starts the same program whatever PATH the engine has.
fn same_program(holder: Option<&Holder>, argv: &mut [String]) {
    let (Some(h), Some(first)) = (holder, argv.first_mut()) else { return };
    let Some(own) = super::proc_scan::argv(h.pid) else { return };
    let rows = super::proc_scan::snapshot();
    let comm = rows.iter().find(|r| r.pid == h.pid).map(|r| r.comm.clone()).unwrap_or_default();
    fn base(p: &str) -> &str {
        p.rsplit('/').next().unwrap_or(p)
    }
    let want = base(first.as_str()).to_string();
    let candidates = [own.first().cloned().unwrap_or_default(), comm];
    if let Some(path) = candidates.iter().find(|p| p.starts_with('/') && base(p.as_str()) == want && std::path::Path::new(p.as_str()).exists()) {
        *first = path.clone();
    }
}

fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.clamp(20, 500), rows.clamp(5, 200))
}

/// Run one adopt call. Move returns at once with the job (it runs on in the background); Copy
/// and New return when the terminal is running; Cancel ends a job still waiting for idle.
pub async fn adopt(req: AdoptRequest, create: Creator) -> Result<Adopted, AdoptError> {
    if ht::harness(&req.harness).is_none() {
        return Err(AdoptError::new(400, "unknown_harness", format!("Unknown harness {:?}.", req.harness)));
    }
    let key: Key = (req.harness.clone(), req.session_id.clone());
    if req.mode != Mode::New && !ht::valid_id(&req.session_id) {
        return Err(AdoptError::new(400, "bad_request", "sessionId is required and must be a plain session id."));
    }
    if req.mode == Mode::Cancel {
        return cancel(&key);
    }
    let guard = lock(&key)?;
    let s = find(req.harness.clone(), req.session_id.clone(), req.address.clone())
        .await
        .ok_or_else(|| AdoptError::new(404, "not_found", "No live session with that harness and id on this computer."))?;
    if s.cwd.is_empty() || !std::path::Path::new(&s.cwd).is_dir() {
        return Err(AdoptError::new(422, "no_cwd", "This session's folder is not known, so it can't be opened in the same place."));
    }
    let (cols, rows) = clamp_size(req.cols, req.rows);
    let external = s.holders.iter().find(|h| h.app != "Allternit");
    let spec = |argv: Vec<String>| CreateSpec { argv, cwd: s.cwd.clone(), title: s.name.clone(), cols, rows };
    match req.mode {
        Mode::Copy => {
            let mut argv = if s.can_copy { ht::copy_argv(&s.harness, &s.session_id, &s.carried_flags) } else { None }
                .ok_or_else(|| AdoptError::new(422, "no_copy", format!("{} can't open a copy of a session.", s.harness)))?;
            same_program(external, &mut argv);
            let line = shell_line(&argv);
            let id = create(spec(argv)).await.map_err(|e| AdoptError::new(502, "start_failed", format!("{e}. Run it by hand in {}: {line}", s.cwd)))?;
            drop(guard);
            Ok(Adopted::Terminal(id))
        }
        Mode::New => {
            let mut argv = ht::new_argv(&s.harness, &s.carried_flags).ok_or_else(|| AdoptError::new(400, "unknown_harness", "Unknown harness."))?;
            same_program(external, &mut argv);
            let line = shell_line(&argv);
            let id = create(spec(argv)).await.map_err(|e| AdoptError::new(502, "start_failed", format!("{e}. Run it by hand in {}: {line}", s.cwd)))?;
            drop(guard);
            Ok(Adopted::Terminal(id))
        }
        Mode::Move => {
            let first_action = decide_move(&s, None)?;
            let first = s.holders[0].clone();
            let mut argv = ht::move_argv(&s.harness, &s.session_id, &s.carried_flags)
                .ok_or_else(|| AdoptError::new(422, "no_resume", format!("{} can't reopen this session by id.", s.harness)))?;
            same_program(Some(&first), &mut argv);
            let job = MoveJob {
                job_id: uuid::Uuid::new_v4().to_string(),
                state: if first_action == Action::WaitIdle { MoveState::WaitingIdle } else { MoveState::Stopping },
                ..Default::default()
            };
            jobs().moves.insert(key.clone(), (job.clone(), None));
            let spec = spec(argv);
            let address = req.address.clone();
            tokio::spawn(run_move(key, job.job_id.clone(), first, spec, address, create, guard));
            Ok(Adopted::Job(job))
        }
        Mode::Cancel => unreachable!("handled above"),
    }
}

fn cancel(key: &Key) -> Result<Adopted, AdoptError> {
    let mut j = jobs();
    let Some((job, done)) = j.moves.get_mut(key) else {
        return Err(AdoptError::new(404, "no_job", "No Move is running for this session."));
    };
    if job.state != MoveState::WaitingIdle {
        return Err(AdoptError::new(409, "too_late", format!("The move is already {}; it can only be cancelled while it waits for the session to go idle.", job.state.as_str())));
    }
    job.state = MoveState::Failed;
    job.code = Some("cancelled".into());
    job.error = Some("Cancelled before anything was stopped.".into());
    *done = Some(Instant::now());
    Ok(Adopted::Cancelled(job.clone()))
}

async fn run_move(key: Key, job_id: String, first: Holder, spec: CreateSpec, address: Option<String>, create: Creator, guard: SessionLock) {
    let _guard = guard;
    let resume = shell_line(&spec.argv);
    let deadline = Instant::now() + WAIT_IDLE_FOR;
    // Recheck right before acting; while busy, look again every 2 s.
    let (pid, started_at) = loop {
        let Some(s) = find(key.0.clone(), key.1.clone(), address.clone()).await else {
            fail(&key, &job_id, &AdoptError::new(409, "session_gone", format!("The session stopped running before it was moved. Resume it in {}: {resume}", spec.cwd)));
            return;
        };
        match decide_move(&s, Some(&first)) {
            Err(e) => return fail(&key, &job_id, &e),
            Ok(Action::Stop { pid, started_at }) => {
                // Only a job still running moves on (a cancel ends it in waiting_idle).
                if !update(&key, &job_id, |j| j.state = MoveState::Stopping) {
                    return;
                }
                break (pid, started_at);
            }
            Ok(Action::WaitIdle) => {
                if !update(&key, &job_id, |j| j.state = MoveState::WaitingIdle) {
                    return;
                }
                if Instant::now() >= deadline {
                    return fail(&key, &job_id, &AdoptError::new(409, "still_busy", "The session was still working after 5 minutes. Nothing was stopped; try again when it is idle."));
                }
                let wake = Instant::now() + WAIT_IDLE_EVERY;
                while Instant::now() < wake {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    if move_job(&key.0, &key.1).map_or(true, |j| j.job_id != job_id || j.state.finished()) {
                        return;
                    }
                }
            }
        }
    };
    let k = key.clone();
    let id = job_id.clone();
    let cwd = spec.cwd.clone();
    let r = stop_then_start(pid, started_at, STOP_TIMEOUT, move || async move {
        update(&k, &id, |j| j.state = MoveState::Starting);
        create(spec).await.map_err(|e| AdoptError::new(502, "start_failed", format!("The old process exited, but the new terminal did not start ({e}). Run it by hand in {cwd}: {resume}")))
    })
    .await;
    match r {
        Ok(terminal_id) => {
            update(&key, &job_id, |j| {
                j.state = MoveState::Done;
                j.terminal_id = Some(terminal_id);
            });
        }
        Err(e) => fail(&key, &job_id, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn holder(pid: u32, app: &str, started: &str) -> Holder {
        Holder { pid, app: app.into(), started_at: (!started.is_empty()).then(|| started.to_string()), ..Default::default() }
    }

    fn session(movable: &str, holders: Vec<Holder>) -> LiveSession {
        LiveSession { harness: "claude".into(), session_id: "s1".into(), movable: movable.into(), holders, ..Default::default() }
    }

    #[test]
    fn adopt_decide_move_per_case() {
        let t = "Sat Nov 21 03:55:44 2026";
        let one = vec![holder(100, "Terminal", t)];
        assert_eq!(decide_move(&session("now", one.clone()), None), Ok(Action::Stop { pid: 100, started_at: t.into() }));
        assert_eq!(decide_move(&session("busy", one.clone()), None), Ok(Action::WaitIdle));
        let two = vec![holder(100, "Terminal", t), holder(101, "iTerm2", t)];
        assert_eq!(decide_move(&session("open_elsewhere", two), None).unwrap_err().code, "session_open_elsewhere");
        let e = decide_move(&session("here", vec![holder(100, "Allternit", t)]), None).unwrap_err();
        assert_eq!((e.status, e.code), (409, "already_here"));
        let e = decide_move(&session("no_resume", one.clone()), None).unwrap_err();
        assert_eq!((e.status, e.code), (422, "no_resume"));
        // An unidentified process of the same CLI may hold it.
        assert_eq!(decide_move(&session("open_elsewhere", vec![]), None).unwrap_err().code, "session_open_elsewhere");
        assert_eq!(decide_move(&session("now", vec![]), None).unwrap_err().code, "no_holder");
        assert_eq!(decide_move(&session("now", vec![holder(100, "Terminal", "")]), None).unwrap_err().code, "holder_unverified");
    }

    #[test]
    fn adopt_decide_move_refuses_a_replaced_holder() {
        let first = holder(100, "Terminal", "Sat Nov  1 03:55:44 2026");
        // Same pid, same start (padding differs): still the same process.
        let same = session("now", vec![holder(100, "Terminal", "Sat Nov 1 03:55:44 2026")]);
        assert!(matches!(decide_move(&same, Some(&first)), Ok(Action::Stop { pid: 100, .. })));
        // Pid reused by a newer process.
        let reused = session("now", vec![holder(100, "Terminal", "Sat Nov 1 04:10:00 2026")]);
        assert_eq!(decide_move(&reused, Some(&first)).unwrap_err().code, "holder_changed");
        let other = session("now", vec![holder(200, "Terminal", "Sat Nov 1 03:55:44 2026")]);
        assert_eq!(decide_move(&other, Some(&first)).unwrap_err().code, "holder_changed");
    }

    #[test]
    fn adopt_start_matches_local_utc_and_rfc3339() {
        let lstart = "Fri Nov 20 21:55:44 2026";
        assert!(start_matches("Fri Nov 20 21:55:44 2026", lstart));
        assert!(!start_matches("Fri Nov 20 21:55:45 2026", lstart));
        assert!(!start_matches("", lstart));
        // UTC in the same shape (Claude Code's procStart) and RFC 3339 depend on the local zone.
        let local = chrono::NaiveDateTime::parse_from_str(lstart, "%a %b %d %H:%M:%S %Y").unwrap();
        let utc = chrono::TimeZone::from_local_datetime(&chrono::Local, &local).single().unwrap().with_timezone(&chrono::Utc);
        assert!(start_matches(&utc.format("%a %b %e %H:%M:%S %Y").to_string(), lstart));
        assert!(start_matches(&(utc + chrono::Duration::milliseconds(800)).to_rfc3339(), lstart));
        assert!(!start_matches(&(utc + chrono::Duration::minutes(5)).to_rfc3339(), lstart));
    }

    /// A child this test owns, reaped on its own thread so an exit is seen (not left a zombie).
    #[cfg(unix)]
    struct Child {
        pid: u32,
        waiter: Option<std::thread::JoinHandle<()>>,
    }

    #[cfg(unix)]
    fn spawn(cmd: &str, args: &[&str]) -> Child {
        let mut c = std::process::Command::new(cmd).args(args).spawn().unwrap();
        let pid = c.id();
        let waiter = std::thread::spawn(move || {
            let _ = c.wait();
        });
        // Let a shell install its trap before anything signals it.
        std::thread::sleep(Duration::from_millis(300));
        Child { pid, waiter: Some(waiter) }
    }

    #[cfg(unix)]
    impl Drop for Child {
        fn drop(&mut self) {
            // SIGKILL only ever goes to this test's own child.
            if probe(self.pid) != Probe::Gone {
                unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
            }
            if let Some(w) = self.waiter.take() {
                let _ = w.join();
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopt_stop_then_start_exits_then_starts() {
        let child = spawn("sleep", &["60"]);
        let started = start_time(child.pid).expect("ps start time of the test child");
        let r = stop_then_start(child.pid, started, Duration::from_secs(5), || async { Ok("term-1".to_string()) }).await;
        assert_eq!(r, Ok("term-1".to_string()));
        assert_eq!(probe(child.pid), Probe::Gone);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopt_stop_then_start_did_not_exit_starts_nothing() {
        let child = spawn("sh", &["-c", "trap '' TERM; while :; do sleep 1; done"]);
        let started = start_time(child.pid).expect("ps start time of the test child");
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        let r = stop_then_start(child.pid, started, Duration::from_millis(1500), move || async move {
            r2.store(true, Ordering::SeqCst);
            Ok("term-x".to_string())
        })
        .await;
        let e = r.unwrap_err();
        assert_eq!(e.code, "did_not_exit");
        assert!(e.message.contains("Nothing was started"), "{}", e.message);
        assert!(!ran.load(Ordering::SeqCst), "nothing may start when the old process is still running");
        assert!(matches!(probe(child.pid), Probe::Running(_)), "the TERM-ignoring child is still running (no SIGKILL)");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopt_stop_refuses_a_pid_whose_start_time_differs() {
        let child = spawn("sleep", &["60"]);
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        let r = stop_then_start(child.pid, "Mon Jan  1 00:00:00 2001".into(), Duration::from_secs(2), move || async move {
            r2.store(true, Ordering::SeqCst);
            Ok(String::new())
        })
        .await;
        assert_eq!(r.unwrap_err().code, "holder_changed");
        assert!(!ran.load(Ordering::SeqCst));
        assert!(matches!(probe(child.pid), Probe::Running(_)), "a pid that failed the start-time check is never signalled");
    }

    #[test]
    fn adopt_lock_is_one_per_session_and_cancel_only_while_waiting() {
        let key: Key = ("claude".into(), format!("lock-test-{}", uuid::Uuid::new_v4()));
        let held = lock(&key).unwrap();
        assert_eq!(lock(&key).err().map(|e| (e.status, e.code)), Some((409, "adopt_in_progress")));
        drop(held);
        assert!(lock(&key).is_ok());

        assert_eq!(cancel(&key).unwrap_err().code, "no_job");
        let job = MoveJob { job_id: "j1".into(), state: MoveState::Stopping, ..Default::default() };
        jobs().moves.insert(key.clone(), (job, None));
        assert_eq!(cancel(&key).unwrap_err().code, "too_late");
        update(&key, "j1", |j| j.state = MoveState::WaitingIdle);
        let Ok(Adopted::Cancelled(j)) = cancel(&key) else { panic!("cancel while waiting_idle") };
        assert_eq!((j.state, j.code.as_deref()), (MoveState::Failed, Some("cancelled")));
        // A cancelled job never moves on to stopping.
        assert!(!update(&key, "j1", |j| j.state = MoveState::Stopping));
        let mut list = vec![LiveSession { harness: key.0.clone(), session_id: key.1.clone(), ..Default::default() }];
        annotate(&mut list);
        assert_eq!(list[0].move_job.as_ref().map(|j| j.state), Some(MoveState::Failed));
    }
}
