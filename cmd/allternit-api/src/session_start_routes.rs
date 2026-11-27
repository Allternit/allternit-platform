//! Start anywhere (contract `docs/contracts/session-start/`): another device picks this computer
//! and a folder and starts a session here, through the cloud relay
//! (`/api/v1/runtime-devices/:id/proxy`).
//!
//! - `GET /api/v1/session-start/options`: folders (Gizzi engine projects, live-session and
//!   terminal cwds, home), per-runtime permission modes, the default runtime.
//! - `POST /api/v1/session-start` (`Idempotency-Key`): runtime `allternit` is forwarded to the
//!   local Gizzi engine (`POST /v1/session-start`, its 201 is returned as is); every other CLI
//!   opens a pane terminal running it, with `ALLTERNIT_SESSION_START_DEPTH` / `_MODE` in its env
//!   so `gizzi start` inside it keeps the depth and mode caps.
//!
//! The checks (safety switch, mode cap, bypass confirm, depth, rate limit, replay) are
//! `allternit_terminal_service::start`; the per-CLI flags are the Factory harness table.
//!
//! While paired, the options snapshot (folders + git remotes, modes) is PUT to cloud-api
//! (`PUT /api/v1/runtime-devices/:id/start-options`, the device credential) at boot and when it
//! changes, at most every 10 minutes, so a phone still sees the folders while this computer
//! sleeps.
//!
//! Safety switch (D6): Desktop holds the setting and drops relayed starts at its relay. Here it
//! is [`remote_start_enabled`]: off when `ALLTERNIT_REMOTE_START` is `0|false|off`, or while the
//! file [`remote_start_off_file`] exists (Desktop writes or removes it when the switch changes).
//! Off refuses relayed starts, relayed terminal create and adopt `new`/`copy`.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use allternit_terminal_service::start::{
    build_options, FolderCandidate, Mode, OptionsInput, Planned, RuntimeModes, StartArgvFn, StartCtx, StartDevice,
    StartError, StartFolder, StartGuards, StartOptions, StartPlan, StartRequest, StartResponse, ENGINE_RUNTIME,
};

use crate::AppState;

/// Set on a terminal-kind session's process: the new session's depth (`origin.depth`, 0 for a
/// person's start).
pub const DEPTH_ENV: &str = "ALLTERNIT_SESSION_START_DEPTH";
/// Set on a terminal-kind session's process: the mode it started in (`auto|default|plan|bypass`).
pub const MODE_ENV: &str = "ALLTERNIT_SESSION_START_MODE";
/// Cloud-api route for the snapshot (`:id` = this runtime).
pub const SNAPSHOT_PATH: &str = "/api/v1/runtime-devices/{id}/start-options";
/// Snapshot pushes at most this often (unless nothing was pushed yet).
pub const SNAPSHOT_EVERY: Duration = Duration::from_secs(600);
const TERMINAL_COLS: u16 = 120;
const TERMINAL_ROWS: u16 = 32;
/// How long a CLI gets to draw its prompt before a prompt it takes no flag for is typed in.
const TYPE_PROMPT_AFTER: Duration = Duration::from_millis(2500);

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/session-start/options", get(options_route))
        .route("/session-start", post(start_route))
}

/// One process-wide rate limit + idempotency map.
fn guards() -> &'static StartGuards {
    static G: OnceLock<StartGuards> = OnceLock::new();
    G.get_or_init(StartGuards::new)
}

// ───────────────────────────────────────────────────────────── safety switch + caller

/// The marker file that turns the safety switch off (`<data dir>/allternit/remote-start-off`).
pub fn remote_start_off_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("allternit").join("remote-start-off"))
}

/// The safety switch (D6), default on.
pub fn remote_start_enabled() -> bool {
    let env_off = std::env::var("ALLTERNIT_REMOTE_START")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(false);
    !env_off && !remote_start_off_file().is_some_and(|f| f.exists())
}

/// The request came from another device: replayed by a relay (`x-allternit-relayed`), proxied,
/// from a non-loopback peer, or carrying a cloud relay signature (`RelayedAuth`'s header).
pub fn is_relayed(headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    headers.contains_key(crate::relay_auth::SIG_HEADER)
        || crate::files_scope::caller(headers, peer) != crate::files_scope::Caller::Local
}

/// `403 remote_start_off` when a relayed caller asks while the switch is off.
pub fn refuse_if_off(headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<Response> {
    (is_relayed(headers, peer) && !remote_start_enabled()).then(|| error_response(StartError::REMOTE_START_OFF))
}

fn error_response(e: StartError) -> Response {
    (status_of(e.status), Json(e.body())).into_response()
}

fn status_of(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

// ───────────────────────────────────────────────────────────── what this computer offers

/// What the start check needs from this computer.
pub struct Facts {
    pub device: StartDevice,
    pub installed: HashSet<String>,
    pub signed_out: HashSet<String>,
    pub modes: BTreeMap<String, RuntimeModes>,
    pub folders: Vec<StartFolder>,
    pub start_argv: StartArgvFn,
}

fn device() -> StartDevice {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    StartDevice {
        runtime_id: crate::relay_auth::process_secret().runtime_id().unwrap_or_else(|| "local".to_string()),
        name: hostname::get().ok().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|| "This computer".to_string()),
        os: os.to_string(),
        remote_start: remote_start_enabled(),
    }
}

fn parse_time(s: Option<&str>) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(s?).ok().map(SystemTime::from)
}

/// The Gizzi engine's folders (`GET /v1/session-start/options`, PR #3178). `None`: the engine is
/// not answering, so the `allternit` runtime is not offered.
async fn engine_folders(headers: &HeaderMap) -> Option<Vec<FolderCandidate>> {
    let url = format!("{}/v1/session-start/options", crate::agent_session_routes::gizzi_base());
    let res = crate::agent_session_routes::gizzi_client(headers).get(url).timeout(Duration::from_secs(3)).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let body: Value = res.json().await.ok()?;
    Some(
        body["folders"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|f| {
                        Some(FolderCandidate { path: PathBuf::from(f["path"].as_str()?), last_used: parse_time(f["lastUsedAt"].as_str()) })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// `cwd` of every entry in a list (live sessions, terminals).
fn cwds(list: &Value) -> Vec<FolderCandidate> {
    list.as_array()
        .map(|l| l.iter().filter_map(|s| s["cwd"].as_str()).filter(|c| !c.is_empty()).map(|c| FolderCandidate { path: PathBuf::from(c), last_used: None }).collect())
        .unwrap_or_default()
}

/// The program is on `PATH` (for CLIs the detector has no probe for: kimi, opencode, qwen).
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            dir.join(program).is_file() || (cfg!(windows) && dir.join(format!("{program}.exe")).is_file())
        })
    })
}

/// Installed and signed-out runtime ids (registry ids), from the same detection as
/// `GET /runtimes` (`cli_catalog::installed_map`), plus CLIs found on `PATH`.
async fn installed_runtimes(engine_up: bool) -> (HashSet<String>, HashSet<String>) {
    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;
    let status = tokio::task::spawn_blocking(move || crate::provider_routes::cli_status_by_id(&connected)).await.unwrap_or_default();
    let map = crate::cli_catalog::installed_map(&status);
    let mut installed = HashSet::new();
    let mut signed_out = HashSet::new();
    for (id, entry) in map.as_object().into_iter().flatten() {
        if id == ENGINE_RUNTIME {
            continue;
        }
        if entry["installed"].as_bool() == Some(true) {
            installed.insert(id.clone());
            if entry["signedIn"].as_bool() == Some(false) {
                signed_out.insert(id.clone());
            }
        } else if allternit_factory_engine::agents::harness_table::start_argv(id, None, None)
            .is_some_and(|(argv, _)| argv.first().is_some_and(|p| on_path(p)))
        {
            installed.insert(id.clone());
        }
    }
    if engine_up {
        installed.insert(ENGINE_RUNTIME.to_string());
    }
    (installed, signed_out)
}

/// Everything the options and the start check need.
async fn gather(state: &AppState, headers: &HeaderMap) -> (StartOptions, Facts) {
    let dev = device();
    let (engine, live, terms) = tokio::join!(
        engine_folders(headers),
        crate::netops::presence::live_sessions(&dev.runtime_id),
        state.terminal_sessions.list(None),
    );
    let engine_up = engine.is_some();
    let mut candidates = engine.unwrap_or_default();
    candidates.extend(cwds(&live));
    if let Ok(t) = terms {
        candidates.extend(cwds(&t["terminals"]));
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    candidates.push(FolderCandidate { path: home.clone(), last_used: None });
    let (installed, signed_out) = installed_runtimes(engine_up).await;
    let mut runtimes: Vec<String> = installed.iter().cloned().collect();
    runtimes.sort();
    let input = OptionsInput {
        device: dev.clone(),
        candidates,
        runtimes,
        home,
        mode_flags: allternit_factory_engine::agents::harness_table::mode_flags,
    };
    let options = tokio::task::spawn_blocking(move || build_options(input)).await.unwrap_or_else(|_| StartOptions {
        device: dev.clone(),
        folders: Vec::new(),
        modes: BTreeMap::new(),
        default_runtime: ENGINE_RUNTIME.to_string(),
    });
    let facts = Facts {
        device: options.device.clone(),
        installed,
        signed_out,
        modes: options.modes.clone(),
        folders: options.folders.clone(),
        start_argv: allternit_factory_engine::agents::harness_table::start_argv,
    };
    (options, facts)
}

// ───────────────────────────────────────────────────────────── routes

async fn options_route(
    State(state): State<Arc<AppState>>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    if let Some(refused) = refuse_if_off(&headers, peer.map(|c| c.0)) {
        return refused;
    }
    let (options, _) = gather(&state, &headers).await;
    note_options(&options).await;
    Json(options).into_response()
}

async fn start_route(
    State(state): State<Arc<AppState>>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let relayed = is_relayed(&headers, peer.map(|c| c.0));
    let enabled = remote_start_enabled();
    if relayed && !enabled {
        return error_response(StartError::REMOTE_START_OFF);
    }
    let req: StartRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "bad_request", "message": format!("Expected the session-start body: {e}"), "action": "Send {folder, runtime, origin}." }))).into_response()
        }
    };
    let idem = headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::to_string);
    let (options, facts) = gather(&state, &headers).await;
    let store = state.terminal_sessions.clone();
    let confirm_bypass = req.confirm_bypass;
    let depth = req.origin.depth.unwrap_or(0);
    let (status, reply) = run_start(guards(), idem.as_deref(), relayed, enabled, &req, &facts, |plan, mode| {
        launch(store, headers.clone(), plan, mode, confirm_bypass, depth)
    })
    .await;
    if status == StatusCode::CREATED {
        note_options(&options).await;
    }
    (status, Json(reply)).into_response()
}

/// The start check, the replay and the run, without HTTP. `launch` runs a fresh plan in the
/// effective mode and returns the 201 body; that body is remembered for the `Idempotency-Key`
/// right after it succeeds.
pub async fn run_start<L, Fut>(
    guards: &StartGuards,
    idempotency_key: Option<&str>,
    relayed: bool,
    remote_start_enabled: bool,
    req: &StartRequest,
    facts: &Facts,
    launch: L,
) -> (StatusCode, Value)
where
    L: FnOnce(StartPlan, Mode) -> Fut,
    Fut: Future<Output = Result<Value, (StatusCode, Value)>>,
{
    let device_id = facts.device.runtime_id.as_str();
    // The context borrows and holds a `dyn Fn`: keep it out of the awaits below.
    let planned = {
        let ctx = StartCtx {
            remote_start_enabled,
            relayed,
            caller_mode: None,
            installed: &facts.installed,
            signed_out: &facts.signed_out,
            modes: &facts.modes,
            folders: &facts.folders,
            git_resolver: None,
            start_argv: facts.start_argv,
        };
        guards.plan(device_id, idempotency_key, req, &ctx)
    };
    let plan = match planned {
        Err(e) => return (status_of(e.status), e.body()),
        Ok(Planned::Replay(body)) => return (StatusCode::CREATED, body),
        Ok(Planned::Fresh(plan)) => plan,
    };
    let mode = req.mode.unwrap_or_else(|| facts.modes.get(&req.runtime).map(|m| m.default).unwrap_or(Mode::Default));
    match launch(plan, mode).await {
        Ok(body) => {
            if let Some(key) = idempotency_key {
                guards.remember(device_id, key, &body);
            }
            (StatusCode::CREATED, body)
        }
        Err(failed) => failed,
    }
}

/// Runs one plan: the engine kind through the Gizzi engine, the terminal kind in a pane.
async fn launch(
    store: crate::terminal_routes::TerminalSessionStore,
    headers: HeaderMap,
    plan: StartPlan,
    mode: Mode,
    confirm_bypass: bool,
    depth: u32,
) -> Result<Value, (StatusCode, Value)> {
    match plan {
        StartPlan::Engine { directory, mode, prompt, origin } => {
            let mut body = json!({
                "folder": { "path": directory.to_string_lossy() },
                "runtime": ENGINE_RUNTIME,
                "mode": mode.as_str(),
                "confirmBypass": confirm_bypass,
                "origin": origin,
            });
            if let Some(p) = prompt {
                body["prompt"] = json!(p);
            }
            let url = format!("{}/v1/session-start", crate::agent_session_routes::gizzi_base());
            let res = crate::agent_session_routes::gizzi_client(&headers)
                .post(url)
                .timeout(Duration::from_secs(30))
                .json(&body)
                .send()
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, StartError::RUNTIME_MISSING.body()))?;
            let status = res.status().as_u16();
            let reply: Value = res.json().await.unwrap_or_else(|_| StartError::RUNTIME_MISSING.body());
            if (200..300).contains(&status) {
                Ok(reply)
            } else {
                Err((status_of(status), reply))
            }
        }
        StartPlan::Terminal { argv, cwd, title, agent, send_after_start } => {
            let cwd = cwd.to_string_lossy().into_owned();
            let mut request = crate::terminal_routes::agent_request(&argv, &cwd, &title, TERMINAL_COLS, TERMINAL_ROWS);
            request.env.insert(DEPTH_ENV.to_string(), depth.to_string());
            request.env.insert(MODE_ENV.to_string(), mode.as_str().to_string());
            let terminal_id = crate::terminal_routes::create_with(&store, request, Some(argv)).await.map_err(|(status, Json(b))| {
                (status, json!({ "error": "terminal_failed", "message": b.message, "action": "Check that Allternit is running on this computer, then try again." }))
            })?;
            if let Some(text) = send_after_start {
                tokio::spawn(type_prompt(store, terminal_id.clone(), text));
            }
            Ok(serde_json::to_value(StartResponse::Terminal { terminal_id, cwd, agent }).unwrap_or_default())
        }
    }
}

/// Types a first prompt into a CLI that takes none on its command line, then Enter on its own
/// (one write with a newline reads as a paste in some TUIs).
async fn type_prompt(store: crate::terminal_routes::TerminalSessionStore, terminal_id: String, text: String) {
    tokio::time::sleep(TYPE_PROMPT_AFTER).await;
    let _ = store.input(&terminal_id, &text).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = store.input(&terminal_id, "\r").await;
}

// ───────────────────────────────────────────────────────────── snapshot to cloud-api

/// The snapshot body cloud-api stores (`{updatedAt, folders, modes, defaultRuntime}`).
pub fn snapshot_body(options: &StartOptions) -> Value {
    json!({
        "updatedAt": chrono::Utc::now().to_rfc3339(),
        "folders": options.folders,
        "modes": options.modes,
        "defaultRuntime": options.default_runtime,
    })
}

fn snapshot_hash(options: &StartOptions) -> u64 {
    let text = serde_json::to_string(&(&options.folders, &options.modes, &options.default_runtime)).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Push now when nothing was pushed yet, or when the options changed and the last push is at
/// least [`SNAPSHOT_EVERY`] old. Never fails the caller.
async fn note_options(options: &StartOptions) {
    static LAST: Mutex<Option<(u64, Instant)>> = Mutex::new(None);
    let hash = snapshot_hash(options);
    let due = {
        let last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        match *last {
            None => true,
            Some((h, at)) => h != hash && at.elapsed() >= SNAPSHOT_EVERY,
        }
    };
    if !due {
        return;
    }
    match push_snapshot(options).await {
        Ok(true) => *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((hash, Instant::now())),
        Ok(false) => {}
        Err(e) => tracing::debug!("start-options snapshot: {e}"),
    }
}

/// `PUT {cloud}/api/v1/runtime-devices/:id/start-options` with this computer's device credential
/// (`relay_auth::process_secret`, the same one the presence heartbeat signs with). `Ok(false)`:
/// not paired, nothing sent.
pub async fn push_snapshot(options: &StartOptions) -> Result<bool, String> {
    let secret = crate::relay_auth::process_secret();
    let (Some(token), Some(runtime_id)) = (secret.device_token(), secret.runtime_id()) else {
        return Ok(false);
    };
    let url = format!("{}{}", crate::phone_sync::cloud_base().trim_end_matches('/'), SNAPSHOT_PATH.replace("{id}", &urlencoding::encode(&runtime_id)));
    let res = crate::net_client::client()
        .put(url)
        .timeout(Duration::from_secs(20))
        .bearer_auth(token)
        .json(&snapshot_body(options))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.status().is_success() {
        Ok(true)
    } else {
        Err(format!("cloud-api answered {}", res.status()))
    }
}

/// Snapshot loop: once shortly after boot, then every [`SNAPSHOT_EVERY`] (pushes only on change).
/// Inert until paired. Started from `netops::presence::spawn`.
pub fn spawn_snapshot(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            if crate::relay_auth::process_secret().device_token().is_some() {
                let (options, _) = gather(&state, &HeaderMap::new()).await;
                note_options(&options).await;
            }
            tokio::time::sleep(SNAPSHOT_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const REQ_PATH: &str = include_str!("../../../docs/contracts/session-start/start.request.path.json");
    const RES_TERMINAL: &str = include_str!("../../../docs/contracts/session-start/start.response.terminal.json");

    fn fake_start_argv(rt: &str, mode: Option<&str>, prompt: Option<&str>) -> Option<(Vec<String>, bool)> {
        if rt != "claude" {
            return None;
        }
        let mut argv = vec!["claude".to_string()];
        if let Some(m) = mode {
            argv.extend(["--permission-mode".to_string(), m.to_string()]);
        }
        Some(match prompt {
            Some(p) => {
                argv.push(p.to_string());
                (argv, true)
            }
            None => (argv, false),
        })
    }

    fn facts(dir: &Path) -> Facts {
        let mut modes = BTreeMap::new();
        modes.insert("claude".to_string(), RuntimeModes { default: Mode::Default, supported: Mode::ALL.to_vec() });
        Facts {
            device: StartDevice { runtime_id: "rt_test".into(), name: "Test Mac".into(), os: "macos".into(), remote_start: true },
            installed: ["claude".to_string(), ENGINE_RUNTIME.to_string()].into_iter().collect(),
            signed_out: HashSet::new(),
            modes,
            folders: vec![StartFolder { path: dir.to_string_lossy().into_owned(), name: "x".into(), last_used_at: String::new(), git: None }],
            start_argv: fake_start_argv,
        }
    }

    /// The path fixture, pointed at a real folder and at `claude` (terminal kind).
    fn request(dir: &Path, mode: Option<&str>) -> StartRequest {
        let mut v: Value = serde_json::from_str(REQ_PATH).unwrap();
        v["folder"]["path"] = json!(dir.to_string_lossy());
        v["runtime"] = json!("claude");
        match mode {
            Some(m) => v["mode"] = json!(m),
            None => {
                v.as_object_mut().unwrap().remove("mode");
            }
        }
        serde_json::from_value(v).unwrap()
    }

    fn tmp() -> PathBuf {
        std::fs::canonicalize(std::env::temp_dir()).unwrap()
    }

    /// A launcher that answers with the terminal fixture and counts its runs.
    fn fixture_launch(
        runs: &AtomicUsize,
    ) -> impl FnOnce(StartPlan, Mode) -> std::future::Ready<Result<Value, (StatusCode, Value)>> + '_ {
        move |plan, mode| {
            runs.fetch_add(1, Ordering::SeqCst);
            assert!(matches!(plan, StartPlan::Terminal { .. }));
            assert_eq!(mode, Mode::Default);
            std::future::ready(Ok(serde_json::from_str(RES_TERMINAL).unwrap()))
        }
    }

    #[tokio::test]
    async fn session_start_relayed_success_has_the_terminal_shape() {
        let dir = tmp();
        let runs = AtomicUsize::new(0);
        let guards = StartGuards::new();
        let (status, body) = run_start(&guards, None, true, true, &request(&dir, Some("default")), &facts(&dir), fixture_launch(&runs)).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["kind"], "terminal");
        assert!(body["terminalId"].is_string() && body["cwd"].is_string() && body["agent"].is_string());
        assert!(body.get("error").is_none(), "no remote_start_off or other error: {body}");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn session_start_replay_returns_the_same_body_without_a_second_start() {
        let dir = tmp();
        let runs = AtomicUsize::new(0);
        let guards = StartGuards::new();
        let f = facts(&dir);
        let req = request(&dir, None);
        let first = run_start(&guards, Some("key-1"), true, true, &req, &f, fixture_launch(&runs)).await;
        let second = run_start(&guards, Some("key-1"), true, true, &req, &f, fixture_launch(&runs)).await;
        assert_eq!(first.0, StatusCode::CREATED);
        assert_eq!(first, second);
        assert_eq!(runs.load(Ordering::SeqCst), 1, "the replay must not start a second session");
    }

    #[tokio::test]
    async fn session_start_relayed_bypass_without_confirm_is_refused() {
        let dir = tmp();
        let runs = AtomicUsize::new(0);
        let guards = StartGuards::new();
        let (status, body) = run_start(&guards, None, true, true, &request(&dir, Some("bypass")), &facts(&dir), fixture_launch(&runs)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "mode_not_allowed");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn session_start_relayed_with_switch_off_is_refused() {
        let dir = tmp();
        let runs = AtomicUsize::new(0);
        let guards = StartGuards::new();
        let (status, body) = run_start(&guards, None, true, false, &request(&dir, None), &facts(&dir), fixture_launch(&runs)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "remote_start_off");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn session_start_relayed_marker_or_signature_counts_as_relayed() {
        let mut h = HeaderMap::new();
        assert!(!is_relayed(&h, None), "loopback with no marker is local (cfg(test) peer rule)");
        h.insert(crate::cowork_devices_routes::RELAYED_HEADER, "1".parse().unwrap());
        assert!(is_relayed(&h, None));
        let mut s = HeaderMap::new();
        s.insert(crate::relay_auth::SIG_HEADER, "x".parse().unwrap());
        assert!(is_relayed(&s, None));
    }

    #[test]
    fn session_start_snapshot_body_has_the_contract_fields() {
        let dir = tmp();
        let f = facts(&dir);
        let options = StartOptions { device: f.device.clone(), folders: f.folders.clone(), modes: f.modes.clone(), default_runtime: ENGINE_RUNTIME.into() };
        let body = snapshot_body(&options);
        for key in ["updatedAt", "folders", "modes", "defaultRuntime"] {
            assert!(body.get(key).is_some(), "{key} missing");
        }
        assert!(body.get("device").is_none(), "the cloud keeps the device facts itself");
    }
}
