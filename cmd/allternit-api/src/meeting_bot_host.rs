//! Meetings bot host: runs the Meetings bot (services/meeting-bot) on this
//! runtime, the user's cloud computer or Desktop (docs/meetings/CONTRACT.md,
//! "Change 2026-11-14: runtime launcher"). cloud-api's `RuntimePodProvider`
//! relays three signed POSTs here; every route sits behind
//! [`RelayedAuth`] (unsigned = 401, bodies capped at 1 MB).
//!
//! - `POST /webhooks/meeting-bot/status {session_id?}` →
//!   `200 {ready, reason: null|"busy"|"chrome_missing"|"bot_missing", os, max,
//!   sessions: [{session_id, running, exit_code}]}`. A session stays listed
//!   after its runner exits, until `stop` (cloud-api treats a session missing
//!   from an answered list as gone).
//! - `POST /webhooks/meeting-bot/start {session_id, pod_config, pod_token,
//!   livekit_token, bot_profile: {account}|null}` → `200 {status:"started", os}`
//!   or `409 {error: "busy"|"chrome_missing"|"bot_missing"}`. At most
//!   `ALLTERNIT_MEETING_BOT_MAX` (default 1) runners at once. A start for a
//!   session that is already running answers `200` again (a relay retry).
//! - `POST /webhooks/meeting-bot/stop {session_id}` → `200 {stopped}`: kills
//!   the runner's process group, deletes its profile dir, drops the session.
//!
//! The runner is `<node> <bot dir>/dist/runner.js` in its own process group
//! (Windows: its own process group, killed with `taskkill /T /F`):
//! - node = `ALLTERNIT_MEETING_NODE` (`ELECTRON_RUN_AS_NODE=1` only with
//!   `ALLTERNIT_MEETING_NODE_ELECTRON=1`), else `node` on PATH;
//! - bot dir = `ALLTERNIT_MEETING_BOT_DIR`, else `meeting-bot/` next to or one
//!   level above this binary (the runtime package's `resources/bin` layout);
//! - browser = `CHROME_PATH`, else installed Google Chrome, Edge, Chromium
//!   (the same order as the bot's `findChrome`). Never Electron.
//!
//! The child does not inherit this process's environment (device token,
//! keys): it gets the [`crate::env_allowlist`] set, display/OS basics,
//! `MEETING_BOT_*` passthrough and its own values: `ALLTERNIT_POD_CONFIG`
//! (base64 JSON), `ALLTERNIT_POD_TOKEN`, `ALLTERNIT_LIVEKIT_TOKEN`, a fresh
//! random `ALLTERNIT_INTERNAL_SERVICE_TOKEN` (its turn token, registered in
//! [`crate::meeting_tools::register_session_cloud`] so its turns use this
//! session's cloud URL and pod token), `ALLTERNIT_MEETING_TURN_URL`
//! (`http://127.0.0.1:<this api's port>`), `CHROME_PATH` and
//! `MEETING_BOT_WINDOW` (`normal` on a cloud computer: `/etc/allternit/bootstrap.json`
//! and `DISPLAY`; else `headless`). Output goes to
//! `<data_dir>/meeting-bot/logs/<session>.log`.
//!
//! Bot Google account (Meet): Linux only. The host fetches the profile with
//! the pod token (`GET <cloudApiUrl>/api/v1/meeting-sessions/<sid>/bot-profile`,
//! `tar.zst` or 204), unpacks it into `<data_dir>/meeting-bot/profiles/<sha256(account)[..16]>`
//! (0700) and points `identity.profileDir` there; the dir is deleted when the
//! runner exits or is stopped. On macOS and Windows (or with no profile) a
//! `google_account` identity becomes `anonymous`: the bot joins signed out.
//!
//! Audio (Linux): when no PulseAudio server answers `pactl info`, the host
//! starts its own `pulseaudio` with the null sink `meet_out` as default (the
//! old pod image's `meeting.pa`) and gives the child `PULSE_SERVER` /
//! `PULSE_SINK`. Runners and that PulseAudio are killed on stop and on
//! allternit-api shutdown ([`shutdown`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::meeting_tools;
use crate::relay_auth::{RelaySecret, RelayedAuth};

pub const STATUS_PATH: &str = "/webhooks/meeting-bot/status";
pub const START_PATH: &str = "/webhooks/meeting-bot/start";
pub const STOP_PATH: &str = "/webhooks/meeting-bot/stop";

const BOOTSTRAP_MARKER: &str = "/etc/allternit/bootstrap.json";
const RUNNER_REL: &str = "dist/runner.js";
/// SIGTERM, then this long before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(5);
const KILL_WAIT: Duration = Duration::from_secs(3);
const PROFILE_TIMEOUT: Duration = Duration::from_secs(120);
const PULSE_READY_WAIT: Duration = Duration::from_secs(5);

/// The old pod image's `meeting.pa`, with this host's socket.
fn meeting_pa(socket: &Path) -> String {
    format!(
        ".fail\n\
         load-module module-null-sink sink_name=meet_out sink_properties=device.description=meet_out rate=48000\n\
         set-default-sink meet_out\n\
         load-module module-native-protocol-unix auth-anonymous=1 socket={}\n\
         load-module module-always-sink\n",
        socket.display()
    )
}

/// Variables the runner needs beyond [`crate::env_allowlist`]: the display,
/// the session bus, and what Windows programs need to start at all.
const EXTRA_CHILD_ENV: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
];

// ---------------------------------------------------------------- config

/// Where the runner, node and the browser come from, and the limits.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// `ALLTERNIT_MEETING_BOT_DIR`; `None` = next to this binary.
    pub bot_dir: Option<PathBuf>,
    /// `ALLTERNIT_MEETING_NODE`; `None` = `node` on PATH.
    pub node: Option<String>,
    /// `ALLTERNIT_MEETING_NODE_ELECTRON=1`: `node` is Electron.
    pub node_electron: bool,
    /// `ALLTERNIT_MEETING_BOT_MAX`, at least 1.
    pub max: usize,
    pub data_dir: PathBuf,
    pub api_port: u16,
    /// A fixed browser (tests); `None` = [`find_chrome`].
    pub chrome: Option<PathBuf>,
    /// Start PulseAudio when none answers (Linux). Off in tests.
    pub audio: bool,
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl HostConfig {
    pub fn from_env(data_dir: PathBuf, api_port: u16) -> Self {
        Self {
            bot_dir: env_nonempty("ALLTERNIT_MEETING_BOT_DIR").map(PathBuf::from),
            node: env_nonempty("ALLTERNIT_MEETING_NODE"),
            node_electron: env_nonempty("ALLTERNIT_MEETING_NODE_ELECTRON").as_deref() == Some("1"),
            max: env_nonempty("ALLTERNIT_MEETING_BOT_MAX").and_then(|v| v.parse().ok()).unwrap_or(1).max(1),
            data_dir,
            api_port,
            chrome: None,
            audio: true,
        }
    }

    /// The bot dir that has `dist/runner.js`.
    fn bot_dir(&self) -> Option<PathBuf> {
        let candidates: Vec<PathBuf> = match &self.bot_dir {
            Some(dir) => vec![dir.clone()],
            None => {
                let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
                exe_dir.map(|d| vec![d.join("meeting-bot"), d.join("..").join("meeting-bot")]).unwrap_or_default()
            }
        };
        candidates.into_iter().find(|d| d.join(RUNNER_REL).is_file())
    }

    /// `(program, is_electron)`: `ALLTERNIT_MEETING_NODE` when it resolves,
    /// else `node` on PATH (never Electron then).
    fn node(&self) -> Option<(PathBuf, bool)> {
        if let Some(node) = &self.node {
            if let Some(path) = resolve_program(node) {
                return Some((path, self.node_electron));
            }
            warn!(node = %node, "meeting bot: ALLTERNIT_MEETING_NODE not found; trying node on PATH");
        }
        resolve_program("node").map(|p| (p, false))
    }

    fn chrome(&self) -> Option<PathBuf> {
        match &self.chrome {
            Some(path) => path.is_file().then(|| path.clone()),
            None => find_chrome(),
        }
    }

    fn root(&self) -> PathBuf {
        self.data_dir.join("meeting-bot")
    }

    /// `<data_dir>/meeting-bot/profiles/<sha256(account)[..16]>`.
    pub fn profile_dir(&self, account: &str) -> PathBuf {
        let digest = hex::encode(Sha256::digest(account.trim().to_lowercase().as_bytes()));
        self.root().join("profiles").join(&digest[..16])
    }
}

/// A path that exists, or a bare name found on PATH (`.exe` tried on Windows).
fn resolve_program(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    find_on_path(name)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) && !name.to_lowercase().ends_with(".exe") { vec![format!("{name}.exe"), name.to_string()] } else { vec![name.to_string()] };
    std::env::split_paths(&paths).flat_map(|dir| names.iter().map(move |n| dir.join(n))).find(|p| p.is_file())
}

/// `CHROME_PATH`, else the first installed Chrome, Edge, then Chromium
/// (services/meeting-bot `chromeCandidates`).
pub fn find_chrome() -> Option<PathBuf> {
    if let Some(path) = env_nonempty("CHROME_PATH") {
        return Some(PathBuf::from(path)).filter(|p| p.is_file());
    }
    chrome_candidates().into_iter().find(|p| p.is_file())
}

fn chrome_candidates() -> Vec<PathBuf> {
    match std::env::consts::OS {
        "macos" => [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
        .iter()
        .map(PathBuf::from)
        .collect(),
        "windows" => {
            let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].iter().filter_map(|k| env_nonempty(k)).map(PathBuf::from).collect();
            ["Google\\Chrome\\Application\\chrome.exe", "Microsoft\\Edge\\Application\\msedge.exe", "Chromium\\Application\\chrome.exe"]
                .iter()
                .flat_map(|rel| roots.iter().map(move |r| r.join(rel)))
                .collect()
        }
        _ => {
            let dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_else(|| ["/usr/local/bin", "/usr/bin", "/bin"].iter().map(PathBuf::from).collect());
            let on_path = |names: &[&str]| names.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).collect::<Vec<_>>();
            let mut out = on_path(&["google-chrome", "google-chrome-stable"]);
            out.push(PathBuf::from("/opt/google/chrome/chrome"));
            out.extend(on_path(&["microsoft-edge", "microsoft-edge-stable"]));
            out.extend(on_path(&["chromium", "chromium-browser"]));
            out
        }
    }
}

/// `MEETING_BOT_WINDOW` when set, else `normal` on a cloud computer
/// (bootstrap marker + a display), else `headless`.
fn window_mode() -> String {
    if let Some(mode) = env_nonempty("MEETING_BOT_WINDOW").filter(|m| ["headless", "offscreen", "normal"].contains(&m.as_str())) {
        return mode;
    }
    if Path::new(BOOTSTRAP_MARKER).exists() && env_nonempty("DISPLAY").is_some() { "normal" } else { "headless" }.to_string()
}

// ---------------------------------------------------------------- identity + profile

#[derive(Debug, Clone, Deserialize)]
pub struct BotProfile {
    pub account: String,
}

fn set_anonymous(pod_config: &mut Value) {
    pod_config["identity"] = json!({ "kind": "anonymous" });
}

/// Rewrites `pod_config.identity` for this OS: a `google_account` keeps its
/// account only on Linux with a `bot_profile`, with `profileDir` set to the
/// host's per-account dir (returned); otherwise it becomes `anonymous`.
pub fn prepare_identity(pod_config: &mut Value, os: &str, bot_profile: Option<&BotProfile>, cfg: &HostConfig) -> Option<PathBuf> {
    if pod_config["identity"]["kind"] != "google_account" {
        return None;
    }
    match bot_profile.filter(|p| os == "linux" && !p.account.trim().is_empty()) {
        Some(profile) => {
            let dir = cfg.profile_dir(&profile.account);
            pod_config["identity"]["profileDir"] = json!(dir.to_string_lossy());
            Some(dir)
        }
        None => {
            set_anonymous(pod_config);
            None
        }
    }
}

/// Unpack a profile `tar.zst` into `dir` (emptied first, 0700).
pub fn unpack_profile(archive: &[u8], dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    create_private_dir(dir)?;
    let mut tar = tar::Archive::new(zstd::Decoder::new(archive)?);
    tar.set_preserve_ownerships(false);
    tar.unpack(dir)
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// `Ok(true)` = unpacked, `Ok(false)` = no profile (204).
async fn fetch_profile(cloud_url: &str, session_id: &str, pod_token: &str, dir: &Path) -> Result<bool, String> {
    let url = format!("{}/api/v1/meeting-sessions/{session_id}/bot-profile", cloud_url.trim_end_matches('/'));
    let res = crate::net_client::client().get(&url).bearer_auth(pod_token).timeout(PROFILE_TIMEOUT).send().await.map_err(|e| format!("bot-profile request failed: {e}"))?;
    match res.status().as_u16() {
        204 => return Ok(false),
        200 => {}
        other => return Err(format!("bot-profile answered {other}")),
    }
    let bytes = res.bytes().await.map_err(|e| format!("bot-profile body failed: {e}"))?;
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || unpack_profile(&bytes, &dir)).await.map_err(|e| e.to_string())?.map_err(|e| format!("bot-profile unpack failed: {e}"))?;
    Ok(true)
}

fn remove_profile(dir: Option<&Path>) {
    if let Some(dir) = dir {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(dir = %dir.display(), "meeting bot: profile dir not deleted: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------- processes

/// Signal the runner's whole process group (Chrome included). `force` =
/// SIGKILL. Windows: `taskkill /T /F` on the tree.
fn kill_group(pid: u32, force: bool) {
    #[cfg(unix)]
    // SAFETY: killpg only sends a signal; the group was created for this child.
    unsafe {
        libc::killpg(pid as libc::pid_t, if force { libc::SIGKILL } else { libc::SIGTERM });
    }
    #[cfg(windows)]
    {
        let _ = force;
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    status.code().unwrap_or(-1)
}

fn own_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe; it only asks the kernel to SIGTERM
    // the runner if allternit-api dies without stopping it.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}

fn new_turn_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// ---------------------------------------------------------------- host

struct BotSession {
    pid: u32,
    /// `Some(exit code)` once the runner exited.
    exit: watch::Receiver<Option<i32>>,
    profile_dir: Option<PathBuf>,
    turn_token: String,
}

impl BotSession {
    fn running(&self) -> bool {
        self.exit.borrow().is_none()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    Busy,
    ChromeMissing,
    BotMissing,
    Invalid(String),
    Failed(String),
}

impl StartError {
    fn response(&self) -> Response {
        let (status, body) = match self {
            StartError::Busy => (StatusCode::CONFLICT, json!({ "error": "busy" })),
            StartError::ChromeMissing => (StatusCode::CONFLICT, json!({ "error": "chrome_missing" })),
            StartError::BotMissing => (StatusCode::CONFLICT, json!({ "error": "bot_missing" })),
            StartError::Invalid(m) => (StatusCode::BAD_REQUEST, json!({ "error": "invalid_request", "message": m })),
            StartError::Failed(m) => (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "start_failed", "message": m })),
        };
        (status, Json(body)).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub struct StartRequest {
    pub session_id: String,
    pub pod_config: Value,
    pub pod_token: String,
    pub livekit_token: String,
    #[serde(default)]
    pub bot_profile: Option<BotProfile>,
}

pub struct MeetingBotHost {
    cfg: HostConfig,
    sessions: Mutex<HashMap<String, BotSession>>,
    /// Serializes start/stop (each is rare and may wait on a child).
    op_lock: tokio::sync::Mutex<()>,
    /// The PulseAudio this host started, if any.
    pulse: tokio::sync::Mutex<Option<tokio::process::Child>>,
}

impl MeetingBotHost {
    pub fn new(cfg: HostConfig) -> Self {
        Self { cfg, sessions: Mutex::new(HashMap::new()), op_lock: tokio::sync::Mutex::new(()), pulse: tokio::sync::Mutex::new(None) }
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, BotSession>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn blocker(&self) -> Option<StartError> {
        if self.cfg.bot_dir().is_none() || self.cfg.node().is_none() {
            return Some(StartError::BotMissing);
        }
        if self.cfg.chrome().is_none() {
            return Some(StartError::ChromeMissing);
        }
        None
    }

    /// The status reply.
    pub fn status(&self, session_id: Option<&str>) -> Value {
        let blocker = self.blocker();
        let sessions = self.sessions();
        let running = sessions.values().filter(|s| s.running()).count();
        let this_running = session_id.and_then(|sid| sessions.get(sid)).is_some_and(BotSession::running);
        let reason = match blocker {
            Some(StartError::BotMissing) => Some("bot_missing"),
            Some(StartError::ChromeMissing) => Some("chrome_missing"),
            _ if running >= self.cfg.max && !this_running => Some("busy"),
            _ => None,
        };
        let mut list: Vec<Value> = sessions.iter().map(|(sid, s)| json!({ "session_id": sid, "running": s.running(), "exit_code": *s.exit.borrow() })).collect();
        list.sort_by(|a, b| a["session_id"].as_str().cmp(&b["session_id"].as_str()));
        json!({ "ready": reason.is_none(), "reason": reason, "os": std::env::consts::OS, "max": self.cfg.max, "sessions": list })
    }

    pub async fn start(&self, mut req: StartRequest) -> Result<(), StartError> {
        let sid = uuid::Uuid::parse_str(req.session_id.trim()).map_err(|_| StartError::Invalid("session_id must be a UUID".into()))?.to_string();
        if !req.pod_config.is_object() {
            return Err(StartError::Invalid("pod_config must be an object".into()));
        }
        let _op = self.op_lock.lock().await;
        {
            let mut sessions = self.sessions();
            match sessions.get(&sid).map(BotSession::running) {
                Some(true) => return Ok(()),
                Some(false) => {
                    // An exited run of the same session: replaced.
                    if let Some(old) = sessions.remove(&sid) {
                        meeting_tools::unregister_session_cloud(&old.turn_token);
                        remove_profile(old.profile_dir.as_deref());
                    }
                }
                None => {}
            }
            if sessions.values().filter(|s| s.running()).count() >= self.cfg.max {
                return Err(StartError::Busy);
            }
        }
        if let Some(blocker) = self.blocker() {
            return Err(blocker);
        }
        let (bot_dir, (node, electron), chrome) = match (self.cfg.bot_dir(), self.cfg.node(), self.cfg.chrome()) {
            (Some(b), Some(n), Some(c)) => (b, n, c),
            _ => return Err(StartError::BotMissing),
        };

        let os = std::env::consts::OS;
        let cloud_url = req.pod_config["cloudApiUrl"].as_str().unwrap_or_default().to_string();
        let mut profile_dir = prepare_identity(&mut req.pod_config, os, req.bot_profile.as_ref(), &self.cfg);
        if let Some(dir) = profile_dir.clone() {
            match fetch_profile(&cloud_url, &sid, &req.pod_token, &dir).await {
                Ok(true) => info!(%sid, "meeting bot: bot profile unpacked"),
                Ok(false) => {
                    info!(%sid, "meeting bot: no bot profile for this session; joining signed out");
                    set_anonymous(&mut req.pod_config);
                    remove_profile(Some(&dir));
                    profile_dir = None;
                }
                Err(e) => {
                    warn!(%sid, "meeting bot: {e}; joining signed out");
                    set_anonymous(&mut req.pod_config);
                    remove_profile(Some(&dir));
                    profile_dir = None;
                }
            }
        }
        let pulse_env = self.ensure_pulse().await;

        let turn_token = new_turn_token();
        let mut env = crate::env_allowlist::minimal_child_env(None);
        for key in EXTRA_CHILD_ENV {
            if let Some(v) = env_nonempty(key) {
                env.insert((*key).to_string(), v);
            }
        }
        for (k, v) in std::env::vars() {
            if k.starts_with("MEETING_BOT_") {
                env.insert(k, v);
            }
        }
        if electron {
            env.insert("ELECTRON_RUN_AS_NODE".into(), "1".into());
        }
        env.insert("NODE_ENV".into(), "production".into());
        env.insert("ALLTERNIT_POD_CONFIG".into(), base64::engine::general_purpose::STANDARD.encode(req.pod_config.to_string()));
        env.insert("ALLTERNIT_POD_TOKEN".into(), req.pod_token.clone());
        env.insert("ALLTERNIT_LIVEKIT_TOKEN".into(), req.livekit_token.clone());
        env.insert("ALLTERNIT_INTERNAL_SERVICE_TOKEN".into(), turn_token.clone());
        env.insert("ALLTERNIT_MEETING_TURN_URL".into(), format!("http://127.0.0.1:{}", self.cfg.api_port));
        env.insert("CHROME_PATH".into(), chrome.to_string_lossy().into_owned());
        env.insert("MEETING_BOT_WINDOW".into(), window_mode());
        env.extend(pulse_env);

        let logs = self.cfg.root().join("logs");
        let log = create_private_dir(&logs).and_then(|()| std::fs::File::create(logs.join(format!("{sid}.log"))));
        let (stdout, stderr) = match log.and_then(|f| Ok((f.try_clone()?, f))) {
            Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
            Err(e) => {
                warn!(%sid, "meeting bot: no log file ({e}); runner output dropped");
                (Stdio::null(), Stdio::null())
            }
        };
        let mut cmd = tokio::process::Command::new(&node);
        cmd.arg(bot_dir.join(RUNNER_REL)).current_dir(&bot_dir).env_clear().envs(&env).stdin(Stdio::null()).stdout(stdout).stderr(stderr);
        own_group(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                remove_profile(profile_dir.as_deref());
                return Err(StartError::Failed(format!("could not start {}: {e}", node.display())));
            }
        };
        let Some(pid) = child.id() else {
            remove_profile(profile_dir.as_deref());
            return Err(StartError::Failed("the runner exited at once".into()));
        };
        meeting_tools::register_session_cloud(&turn_token, &sid, &cloud_url, &req.pod_token);

        let (tx, rx) = watch::channel(None);
        let (wait_sid, wait_profile, wait_token) = (sid.clone(), profile_dir.clone(), turn_token.clone());
        tokio::spawn(async move {
            let code = match child.wait().await {
                Ok(status) => exit_code_of(status),
                Err(_) => -1,
            };
            // Chrome and anything else the runner left in its group.
            kill_group(pid, true);
            info!(sid = %wait_sid, code, "meeting bot: runner exited");
            if wait_profile.is_some() {
                tokio::time::sleep(Duration::from_millis(300)).await;
                remove_profile(wait_profile.as_deref());
            }
            meeting_tools::unregister_session_cloud(&wait_token);
            let _ = tx.send(Some(code));
        });
        info!(%sid, pid, os, "meeting bot: runner started");
        self.sessions().insert(sid, BotSession { pid, exit: rx, profile_dir, turn_token });
        Ok(())
    }

    /// Stop one runner: SIGTERM its group, SIGKILL after [`STOP_GRACE`],
    /// delete its profile dir and drop it. `false` = not known here.
    pub async fn stop(&self, session_id: &str) -> bool {
        let sid = uuid::Uuid::parse_str(session_id.trim()).map(|u| u.to_string()).unwrap_or_else(|_| session_id.trim().to_string());
        let _op = self.op_lock.lock().await;
        let Some(session) = self.sessions().remove(&sid) else { return false };
        end_session(session).await;
        info!(%sid, "meeting bot: stopped");
        true
    }

    /// Stop every runner and the PulseAudio this host started.
    pub async fn stop_all(&self) {
        let _op = self.op_lock.lock().await;
        let all: Vec<BotSession> = self.sessions().drain().map(|(_, s)| s).collect();
        futures::future::join_all(all.into_iter().map(end_session)).await;
        if let Some(mut pulse) = self.pulse.lock().await.take() {
            if let Some(pid) = pulse.id() {
                kill_group(pid, true);
            }
            let _ = pulse.wait().await;
        }
    }

    /// Linux: `PULSE_SERVER` / `PULSE_SINK` of a PulseAudio this host
    /// started because none answered; empty when one already answers.
    async fn ensure_pulse(&self) -> Vec<(String, String)> {
        if std::env::consts::OS != "linux" || !self.cfg.audio {
            return Vec::new();
        }
        let dir = self.cfg.root().join("pulse");
        if let Err(e) = create_private_dir(&dir) {
            warn!("meeting bot: no pulse dir: {e}");
            return Vec::new();
        }
        // The probe must not autospawn a server of its own.
        let client_conf = dir.join("client.conf");
        let _ = std::fs::write(&client_conf, "autospawn = no\n");
        let socket = dir.join("native");
        let ours = vec![("PULSE_SERVER".to_string(), format!("unix:{}", socket.display())), ("PULSE_SINK".to_string(), "meet_out".to_string())];
        let mut pulse = self.pulse.lock().await;
        if let Some(child) = pulse.as_mut() {
            if child.try_wait().ok().flatten().is_none() && pactl_ok(&client_conf, Some(&socket)).await {
                return ours;
            }
            *pulse = None;
        }
        if find_on_path("pactl").is_some() && pactl_ok(&client_conf, None).await {
            return Vec::new();
        }
        let Some(pulseaudio) = find_on_path("pulseaudio") else {
            warn!("meeting bot: no PulseAudio server and no pulseaudio binary; the call has no audio device");
            return Vec::new();
        };
        let pa = dir.join("meeting.pa");
        if let Err(e) = std::fs::write(&pa, meeting_pa(&socket)) {
            warn!("meeting bot: meeting.pa not written: {e}");
            return Vec::new();
        }
        let _ = std::fs::remove_file(&socket);
        let log = std::fs::File::create(dir.join("pulseaudio.log")).ok();
        let mut cmd = tokio::process::Command::new(pulseaudio);
        cmd.args(["--daemonize=no", "--exit-idle-time=-1", "--disallow-exit", "-n", "-F"])
            .arg(&pa)
            .env("XDG_RUNTIME_DIR", &dir)
            .env("PULSE_CLIENTCONFIG", &client_conf)
            .stdin(Stdio::null())
            .stdout(log.as_ref().and_then(|f| f.try_clone().ok()).map(Stdio::from).unwrap_or_else(Stdio::null))
            .stderr(log.map(Stdio::from).unwrap_or_else(Stdio::null));
        own_group(&mut cmd);
        match cmd.spawn() {
            Ok(child) => *pulse = Some(child),
            Err(e) => {
                warn!("meeting bot: pulseaudio did not start: {e}");
                return Vec::new();
            }
        }
        let deadline = tokio::time::Instant::now() + PULSE_READY_WAIT;
        while tokio::time::Instant::now() < deadline {
            if socket.exists() && (find_on_path("pactl").is_none() || pactl_ok(&client_conf, Some(&socket)).await) {
                info!("meeting bot: started PulseAudio with the null sink meet_out");
                return ours;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        warn!("meeting bot: PulseAudio did not come up in time; see {}", dir.join("pulseaudio.log").display());
        ours
    }
}

/// `pactl info` against `socket` (or the default server) without autospawn.
async fn pactl_ok(client_conf: &Path, socket: Option<&Path>) -> bool {
    let mut cmd = tokio::process::Command::new("pactl");
    if let Some(socket) = socket {
        cmd.arg("-s").arg(format!("unix:{}", socket.display()));
    }
    cmd.arg("info").env("PULSE_CLIENTCONFIG", client_conf).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    matches!(tokio::time::timeout(Duration::from_secs(5), cmd.status()).await, Ok(Ok(status)) if status.success())
}

async fn end_session(session: BotSession) {
    if session.running() {
        kill_group(session.pid, false);
        let mut exit = session.exit.clone();
        if tokio::time::timeout(STOP_GRACE, exit.wait_for(Option::is_some)).await.is_err() {
            kill_group(session.pid, true);
            let _ = tokio::time::timeout(KILL_WAIT, exit.wait_for(Option::is_some)).await;
        }
    }
    remove_profile(session.profile_dir.as_deref());
    meeting_tools::unregister_session_cloud(&session.turn_token);
}

// ---------------------------------------------------------------- routes

#[derive(Debug, Default, Deserialize)]
struct SessionBody {
    #[serde(default)]
    session_id: Option<String>,
}

fn session_body(auth: &RelayedAuth) -> Result<SessionBody, Response> {
    if auth.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(SessionBody::default());
    }
    auth.json()
}

async fn status_h(Extension(host): Extension<Arc<MeetingBotHost>>, auth: RelayedAuth) -> Response {
    match session_body(&auth) {
        Ok(body) => Json(host.status(body.session_id.as_deref())).into_response(),
        Err(response) => response,
    }
}

async fn start_h(Extension(host): Extension<Arc<MeetingBotHost>>, auth: RelayedAuth) -> Response {
    let req: StartRequest = match auth.json() {
        Ok(req) => req,
        Err(response) => return response,
    };
    match host.start(req).await {
        Ok(()) => Json(json!({ "status": "started", "os": std::env::consts::OS })).into_response(),
        Err(e) => {
            if !matches!(e, StartError::Busy) {
                warn!("meeting bot: start refused: {e:?}");
            }
            e.response()
        }
    }
}

async fn stop_h(Extension(host): Extension<Arc<MeetingBotHost>>, auth: RelayedAuth) -> Response {
    match session_body(&auth) {
        Ok(SessionBody { session_id: Some(sid) }) => Json(json!({ "stopped": host.stop(&sid).await })).into_response(),
        Ok(_) => StartError::Invalid("session_id is required".into()).response(),
        Err(response) => response,
    }
}

/// The three relayed routes for `host`, verified with `secret`.
pub fn router_with<S: Clone + Send + Sync + 'static>(host: Arc<MeetingBotHost>, secret: Arc<dyn RelaySecret>) -> Router<S> {
    Router::new()
        .route(STATUS_PATH, post(status_h))
        .route(START_PATH, post(start_h))
        .route(STOP_PATH, post(stop_h))
        .layer(Extension(host))
        .layer(crate::relay_auth::secret_layer(secret))
}

static HOST: OnceLock<Arc<MeetingBotHost>> = OnceLock::new();

/// Production: the process-wide host (env config) behind the process relay
/// secret. [`shutdown`] stops its runners.
pub fn meeting_bot_host_router<S: Clone + Send + Sync + 'static>(data_dir: PathBuf, api_port: u16) -> Router<S> {
    let host = HOST.get_or_init(|| Arc::new(MeetingBotHost::new(HostConfig::from_env(data_dir, api_port)))).clone();
    router_with(host, crate::relay_auth::process_secret())
}

/// allternit-api is shutting down: stop every runner and the PulseAudio
/// this host started.
pub async fn shutdown() {
    if let Some(host) = HOST.get() {
        host.stop_all().await;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::relay_auth::{relayed_post, StaticRelaySecret};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TOKEN: &str = "allternit_runtime_test";
    const OWNER: &str = "user-1";
    const SID_A: &str = "11111111-1111-4111-8111-111111111111";
    const SID_B: &str = "22222222-2222-4222-8222-222222222222";

    fn cfg(data_dir: &Path, bot_dir: &Path) -> HostConfig {
        HostConfig {
            bot_dir: Some(bot_dir.to_path_buf()),
            node: Some("/bin/sh".into()),
            node_electron: false,
            max: 1,
            data_dir: data_dir.to_path_buf(),
            api_port: 18999,
            chrome: Some(PathBuf::from("/bin/sh")),
            audio: false,
        }
    }

    /// A stub runner (run with /bin/sh as "node"): records its env and pid,
    /// leaves a grandchild in its group, then runs `body`.
    fn stub_bot(body: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("dist")).unwrap();
        let script = format!(
            "d=$(dirname \"$0\")/..\n\
             printf '%s\\n' \"$ALLTERNIT_POD_TOKEN\" \"$ALLTERNIT_INTERNAL_SERVICE_TOKEN\" \"$ALLTERNIT_MEETING_TURN_URL\" \"$CHROME_PATH\" \"$ALLTERNIT_RUNTIME_DEVICE_TOKEN\" > \"$d/env.out\"\n\
             echo \"$ALLTERNIT_POD_CONFIG\" > \"$d/config.b64\"\n\
             sleep 60 &\n\
             echo $! > \"$d/grandchild\"\n\
             {body}\n"
        );
        std::fs::write(dir.path().join(RUNNER_REL), script).unwrap();
        dir
    }

    fn app(host: Arc<MeetingBotHost>) -> Router {
        router_with(host, Arc::new(StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() }))
    }

    async fn call(app: &Router, path: &str, body: Value, signed: bool) -> (StatusCode, Value) {
        let bytes = body.to_string().into_bytes();
        let res = app.clone().oneshot(relayed_post(path, &bytes, signed.then_some((TOKEN, OWNER)))).await.unwrap();
        let status = res.status();
        let raw = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }

    fn start_body(sid: &str) -> Value {
        json!({
            "session_id": sid,
            "pod_config": { "sessionId": sid, "cloudApiUrl": "https://cloud.example", "identity": { "kind": "google_account", "accountEmail": "bot@x.com", "profileDir": "/var/lib/allternit/bot-profile" } },
            "pod_token": format!("pod-{sid}"),
            "livekit_token": "lk",
            // No profile: a fetch would leave the machine (prepare_identity is tested alone).
            "bot_profile": null,
        })
    }

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    async fn wait_until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..50 {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    #[tokio::test]
    async fn meeting_bot_start_runs_second_start_is_busy_and_stop_kills_the_group() {
        let (data, bot) = (tempfile::tempdir().unwrap(), stub_bot("wait"));
        let app = app(Arc::new(MeetingBotHost::new(cfg(data.path(), bot.path()))));

        let (status, body) = call(&app, START_PATH, start_body(SID_A), true).await;
        assert_eq!((status, body.clone()), (StatusCode::OK, json!({ "status": "started", "os": std::env::consts::OS })));
        assert!(wait_until(|| bot.path().join("grandchild").exists() && bot.path().join("env.out").exists()).await);

        let (_, status_body) = call(&app, STATUS_PATH, json!({ "session_id": SID_B }), true).await;
        assert_eq!(status_body["sessions"], json!([{ "session_id": SID_A, "running": true, "exit_code": null }]));
        assert_eq!((status_body["ready"].clone(), status_body["reason"].clone(), status_body["max"].clone()), (json!(false), json!("busy"), json!(1)));

        let (status, body) = call(&app, START_PATH, start_body(SID_B), true).await;
        assert_eq!((status, body), (StatusCode::CONFLICT, json!({ "error": "busy" })));

        // The child got its own values, not this process's secrets.
        let env = std::fs::read_to_string(bot.path().join("env.out")).unwrap();
        let lines: Vec<&str> = env.lines().collect();
        assert_eq!(lines[0], format!("pod-{SID_A}"));
        assert_eq!(lines[1].len(), 64, "a fresh 32-byte turn token");
        assert_eq!((lines[2], lines[3], lines.get(4).copied().unwrap_or("")), ("http://127.0.0.1:18999", "/bin/sh", ""));
        // No bot profile: the Meet account becomes a signed-out guest.
        let config: Value = serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(std::fs::read_to_string(bot.path().join("config.b64")).unwrap().trim()).unwrap()).unwrap();
        assert_eq!((config["identity"].clone(), config["sessionId"].clone()), (json!({ "kind": "anonymous" }), json!(SID_A)));

        // Its turn token resolves to this session's cloud URL and pod token.
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(meeting_tools::TURN_TOKEN_HEADER, lines[1].parse().unwrap());
        match crate::meeting_turn_stream::session_turn_auth(&headers) {
            Some(crate::meeting_turn_stream::TurnAuth::Session { session_id, cloud }) => {
                assert_eq!(session_id, SID_A);
                assert_eq!(cloud.parts(), ("https://cloud.example", format!("pod-{SID_A}").as_str()));
            }
            _ => panic!("the runner's turn token is not registered"),
        }

        let grandchild: i32 = std::fs::read_to_string(bot.path().join("grandchild")).unwrap().trim().parse().unwrap();
        assert!(alive(grandchild));
        let (status, body) = call(&app, STOP_PATH, json!({ "session_id": SID_A }), true).await;
        assert_eq!((status, body), (StatusCode::OK, json!({ "stopped": true })));
        assert!(wait_until(|| !alive(grandchild)).await, "stop kills the runner's whole group");
        assert!(crate::meeting_turn_stream::session_turn_auth(&headers).is_none(), "the token dies with the session");
        let (_, status_body) = call(&app, STATUS_PATH, json!({}), true).await;
        assert_eq!((status_body["ready"].clone(), status_body["sessions"].clone()), (json!(true), json!([])));
    }

    #[tokio::test]
    async fn meeting_bot_exit_code_is_reported_and_kept_until_stop() {
        let (data, bot) = (tempfile::tempdir().unwrap(), stub_bot("exit 3"));
        let host = Arc::new(MeetingBotHost::new(cfg(data.path(), bot.path())));
        let app = app(host.clone());
        assert_eq!(call(&app, START_PATH, start_body(SID_A), true).await.0, StatusCode::OK);
        assert!(wait_until(|| host.status(None)["sessions"][0]["running"] == json!(false)).await);
        for _ in 0..2 {
            let (_, body) = call(&app, STATUS_PATH, json!({ "session_id": SID_A }), true).await;
            assert_eq!(body["sessions"], json!([{ "session_id": SID_A, "running": false, "exit_code": 3 }]));
            assert_eq!(body["ready"], json!(true), "an exited runner frees the slot");
        }
        assert_eq!(call(&app, STOP_PATH, json!({ "session_id": SID_A }), true).await.1, json!({ "stopped": true }));
        assert_eq!(call(&app, STATUS_PATH, json!({}), true).await.1["sessions"], json!([]));
    }

    #[tokio::test]
    async fn meeting_bot_routes_refuse_unsigned_requests() {
        let (data, bot) = (tempfile::tempdir().unwrap(), stub_bot("wait"));
        let app = app(Arc::new(MeetingBotHost::new(cfg(data.path(), bot.path()))));
        for (path, body) in [(STATUS_PATH, json!({})), (START_PATH, start_body(SID_A)), (STOP_PATH, json!({ "session_id": SID_A }))] {
            assert_eq!(call(&app, path, body, false).await.0, StatusCode::UNAUTHORIZED, "{path}");
        }
        assert!(!bot.path().join("env.out").exists(), "nothing started");
    }

    #[tokio::test]
    async fn meeting_bot_missing_runner_or_browser_is_not_ready() {
        let data = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let app_missing = app(Arc::new(MeetingBotHost::new(cfg(data.path(), empty.path()))));
        assert_eq!(call(&app_missing, STATUS_PATH, json!({}), true).await.1["reason"], json!("bot_missing"));
        assert_eq!(call(&app_missing, START_PATH, start_body(SID_A), true).await, (StatusCode::CONFLICT, json!({ "error": "bot_missing" })));
        let bot = stub_bot("wait");
        let mut no_chrome = cfg(data.path(), bot.path());
        no_chrome.chrome = Some(data.path().join("no-such-chrome"));
        let app_no_chrome = app(Arc::new(MeetingBotHost::new(no_chrome)));
        assert_eq!(call(&app_no_chrome, STATUS_PATH, json!({}), true).await.1["reason"], json!("chrome_missing"));
        assert_eq!(call(&app_no_chrome, START_PATH, start_body(SID_A), true).await, (StatusCode::CONFLICT, json!({ "error": "chrome_missing" })));
        assert!(!bot.path().join("env.out").exists(), "nothing started");
    }

    #[test]
    fn meeting_bot_identity_keeps_the_account_only_on_linux() {
        let data = tempfile::tempdir().unwrap();
        let c = cfg(data.path(), data.path());
        let profile = BotProfile { account: "Bot@X.com".into() };
        let base = json!({ "identity": { "kind": "google_account", "accountEmail": "bot@x.com", "profileDir": "/var/lib/allternit/bot-profile" } });
        let mut linux = base.clone();
        let dir = prepare_identity(&mut linux, "linux", Some(&profile), &c).unwrap();
        assert_eq!(dir, c.profile_dir("bot@x.com"));
        assert_eq!(dir.file_name().unwrap().len(), 16);
        assert_eq!(linux["identity"]["profileDir"], json!(dir.to_string_lossy()));
        for (os, p) in [("macos", Some(&profile)), ("windows", Some(&profile)), ("linux", None)] {
            let mut v = base.clone();
            assert!(prepare_identity(&mut v, os, p, &c).is_none());
            assert_eq!(v["identity"], json!({ "kind": "anonymous" }), "{os}");
        }
    }

    #[test]
    fn meeting_bot_profile_archive_unpacks_into_a_private_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tar = tar::Builder::new(Vec::new());
        let data = b"{\"profile\":{}}";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "Default/Preferences", &data[..]).unwrap();
        let archive = zstd::encode_all(&tar.into_inner().unwrap()[..], 3).unwrap();
        let dir = tmp.path().join("profiles").join("abcd");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stale"), b"old").unwrap();
        unpack_profile(&archive, &dir).unwrap();
        assert_eq!(std::fs::read(dir.join("Default/Preferences")).unwrap(), data);
        assert!(!dir.join("stale").exists(), "the dir is emptied first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn meeting_turn_tokens_resolve_to_their_own_session() {
        meeting_tools::register_session_cloud("tok-one", SID_A, "https://a.example/", "pod-a");
        meeting_tools::register_session_cloud("tok-two", SID_B, "https://b.example", "pod-b");
        let (sid, cloud) = meeting_tools::session_cloud("tok-two").unwrap();
        assert_eq!((sid.as_str(), cloud.parts()), (SID_B, ("https://b.example", "pod-b")));
        let (sid, cloud) = meeting_tools::session_cloud("tok-one").unwrap();
        assert_eq!((sid.as_str(), cloud.parts()), (SID_A, ("https://a.example", "pod-a")));
        assert!(meeting_tools::session_cloud("tok-three").is_none());
        assert!(meeting_tools::session_cloud("").is_none());
        meeting_tools::unregister_session_cloud("tok-one");
        assert!(meeting_tools::session_cloud("tok-one").is_none());
        assert!(meeting_tools::session_cloud("tok-two").is_some());
        meeting_tools::unregister_session_cloud("tok-two");
    }
}
