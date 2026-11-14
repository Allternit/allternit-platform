//! `allternit-brain-runner`: the first process in a person's sealed runner
//! box (plan `PLAN-brain-runner-2026-11-14.md`, D3–D8). It runs the vendor
//! CLIs unmodified and does exactly five things for cloud-api over one
//! outbound socket (contract section 9.7): start a sign-in, take a pasted
//! code, report sign-in status, sign out, and run one tools-off chat turn.
//!
//! - Dials `ALLTERNIT_RUNNER_URL` with `Authorization: Bearer
//!   ALLTERNIT_RUNNER_TOKEN`; no inbound port, no file, terminal, tool or
//!   HTTP routes. Sends `hello`, answers cloud-api's `ping` text frame
//!   (every 30 s) with a `pong` carrying the same `id`, reconnects with backoff.
//! - Each vendor has its own home under `ALLTERNIT_RUNNER_HOME`
//!   (`/home/runner/<vendor>`), so a logout deletes exactly one sign-in.
//! - History is replayed every turn (D8): claude reads stream-json turns on
//!   stdin; codex, grok and kimi get a transcript preamble.
//! - One turn at a time; a second gets `bad_request`. A turn is killed on
//!   `cancel`, on socket loss, or after `ALLTERNIT_RUNNER_TURN_TIMEOUT_SECS`.
//!
//! Environment:
//! `ALLTERNIT_RUNNER_URL`, `ALLTERNIT_RUNNER_TOKEN` (required; cloud-api
//! sets both, plus `HOME=/home/runner`, when it makes the box),
//! `ALLTERNIT_RUNNER_HOME` (default `/home/runner`, the box's `HOME` volume),
//! `ALLTERNIT_RUNNER_VENDORS` (default `claude`: step 0.2 proved only claude
//! live; add the others as each passes a live turn),
//! `ALLTERNIT_RUNNER_TURN_TIMEOUT_SECS` (default 180),
//! `ALLTERNIT_RUNNER_CODEX_CATALOG` (default on; `0` refuses ChatGPT turns
//! instead of running codex without the tools-off catalog).
//! Development only: `ALLTERNIT_RUNNER_BIN_<VENDOR>` (program path) and
//! `ALLTERNIT_RUNNER_<VENDOR>_HOME` (a home outside the runner home, which
//! logout never deletes).

mod process;
mod protocol;
mod stream;
mod vendors;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{header::AUTHORIZATION, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use protocol::ServerFrame;
pub use vendors::Vendor;
use vendors::{Msg, SignIn};

pub const RUNNER_VERSION: &str = "0.1.0";
const MAX_FRAME: usize = 1 << 20;
const LOGIN_EXPIRES: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct Config {
    pub socket_url: String,
    pub token: String,
    pub home: PathBuf,
    pub vendors: Vec<Vendor>,
    pub turn_timeout: Duration,
    pub codex_catalog: bool,
    pub bins: HashMap<Vendor, PathBuf>,
    pub home_overrides: HashMap<Vendor, PathBuf>,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        Self::from_lookup(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        let socket_url = get("ALLTERNIT_RUNNER_URL").ok_or("ALLTERNIT_RUNNER_URL is not set")?;
        if !(socket_url.starts_with("wss://") || socket_url.starts_with("ws://")) {
            return Err("ALLTERNIT_RUNNER_URL must be a ws:// or wss:// URL".into());
        }
        let token = get("ALLTERNIT_RUNNER_TOKEN").ok_or("ALLTERNIT_RUNNER_TOKEN is not set")?;
        let home = PathBuf::from(get("ALLTERNIT_RUNNER_HOME").unwrap_or_else(|| "/home/runner".into()));
        let mut vendors = Vec::new();
        for id in get("ALLTERNIT_RUNNER_VENDORS").unwrap_or_else(|| "claude".into()).split(',') {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            let vendor = Vendor::parse(id).ok_or_else(|| format!("unknown vendor {id:?} in ALLTERNIT_RUNNER_VENDORS"))?;
            if !vendors.contains(&vendor) {
                vendors.push(vendor);
            }
        }
        let turn_timeout = match get("ALLTERNIT_RUNNER_TURN_TIMEOUT_SECS") {
            Some(v) => Duration::from_secs(
                v.trim().parse::<u64>().ok().filter(|n| *n > 0).ok_or("ALLTERNIT_RUNNER_TURN_TIMEOUT_SECS must be a positive number")?,
            ),
            None => Duration::from_secs(180),
        };
        let codex_catalog = get("ALLTERNIT_RUNNER_CODEX_CATALOG").map_or(true, |v| v.trim() != "0");
        let mut bins = HashMap::new();
        let mut home_overrides = HashMap::new();
        for vendor in Vendor::ALL {
            if let Some(bin) = get(&format!("ALLTERNIT_RUNNER_BIN_{}", vendor.env_key())) {
                bins.insert(vendor, PathBuf::from(bin));
            }
            if let Some(dir) = get(&format!("ALLTERNIT_RUNNER_{}_HOME", vendor.env_key())) {
                home_overrides.insert(vendor, PathBuf::from(dir));
            }
        }
        Ok(Config { socket_url, token, home, vendors, turn_timeout, codex_catalog, bins, home_overrides })
    }
}

struct Runner {
    cfg: Config,
    /// The running turn: its id and the token that stops it.
    turn: Mutex<Option<(String, CancellationToken)>>,
}

/// Runs until `shutdown` is cancelled, reconnecting whenever the socket drops.
pub async fn run(cfg: Config, shutdown: CancellationToken) {
    for vendor in &cfg.vendors {
        let _ = std::fs::create_dir_all(vendors::home(&cfg, *vendor));
    }
    let runner = Arc::new(Runner { cfg, turn: Mutex::new(None) });
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match connect_once(&runner, &shutdown).await {
            Ok(()) => info!("runner socket closed"),
            Err(error) => warn!(%error, "runner socket failed"),
        }
        if shutdown.is_cancelled() {
            break;
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        // Up to 25% jitter so a host restart does not bring every box back at once.
        let jitter = backoff.mul_f64(rand::random::<f64>() * 0.25);
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(backoff + jitter) => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
    // No sign-in process outlives the runner.
    for vendor in Vendor::ALL {
        crate::cli_tools::stop_login(&login_key(vendor)).await;
    }
}

async fn hello(runner: &Runner) -> Value {
    let checks = runner.cfg.vendors.iter().map(|vendor| async move {
        let state = match process::sign_in_state(&runner.cfg, *vendor).await {
            Ok(SignIn::In { .. }) => "signed_in",
            Ok(SignIn::Out) => "signed_out",
            Err(_) if vendors::binary(&runner.cfg, *vendor).is_none() => "missing",
            Err(_) => "unknown",
        };
        (vendor.id(), state)
    });
    let states: BTreeMap<&str, &str> = futures::future::join_all(checks).await.into_iter().collect();
    json!({"type": "hello", "runner": RUNNER_VERSION, "vendors": states})
}

async fn connect_once(runner: &Arc<Runner>, shutdown: &CancellationToken) -> Result<(), String> {
    let mut request = runner.cfg.socket_url.as_str().into_client_request().map_err(|e| e.to_string())?;
    let bearer = HeaderValue::from_str(&format!("Bearer {}", runner.cfg.token)).map_err(|_| "bad token".to_string())?;
    request.headers_mut().insert(AUTHORIZATION, bearer);
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_FRAME);
    config.max_frame_size = Some(MAX_FRAME);
    let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
    let (socket, _) = tokio::time::timeout(Duration::from_secs(20), connect)
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|e| e.to_string())?;
    let (mut sink, mut stream) = socket.split();
    sink.send(Message::Text(hello(runner).await.to_string())).await.map_err(|e| e.to_string())?;
    info!("runner socket up");

    // Every reply goes through one writer; handlers only hold a sender.
    let (out_tx, mut out_rx) = mpsc::channel::<Value>(256);
    let (raw_tx, mut raw_rx) = mpsc::channel::<Message>(16);
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                frame = out_rx.recv() => match frame { Some(frame) => Message::Text(frame.to_string()), None => break },
                raw = raw_rx.recv() => match raw { Some(raw) => raw, None => break },
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let connection = shutdown.child_token();
    let mut keepalive = tokio::time::interval(Duration::from_secs(30));
    keepalive.tick().await;
    let mut last_heard = Instant::now();
    let result = loop {
        tokio::select! {
            _ = connection.cancelled() => break Ok(()),
            _ = keepalive.tick() => {
                if last_heard.elapsed() > Duration::from_secs(90) {
                    break Err("nothing from the server for 90 s".into());
                }
                let _ = raw_tx.send(Message::Ping(Vec::new())).await;
            }
            message = stream.next() => {
                last_heard = Instant::now();
                match message {
                    None => break Ok(()),
                    Some(Err(e)) => break Err(e.to_string()),
                    Some(Ok(Message::Text(text))) => handle(runner, &text, &out_tx, &connection),
                    Some(Ok(Message::Ping(payload))) => { let _ = raw_tx.send(Message::Pong(payload)).await; }
                    Some(Ok(Message::Close(_))) => break Ok(()),
                    Some(Ok(_)) => {}
                }
            }
        }
    };
    // A turn never outlives its socket: cloud-api has already closed it.
    connection.cancel();
    drop(out_tx);
    drop(raw_tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    result
}

fn vendor_for(runner: &Runner, id: &str, vendor: &str, out: &mpsc::Sender<Value>) -> Option<Vendor> {
    match Vendor::parse(vendor).filter(|v| runner.cfg.vendors.contains(v)) {
        Some(v) => Some(v),
        None => {
            let _ = out.try_send(protocol::error(Some(id), "bad_request", "That plan is not offered here"));
            None
        }
    }
}

fn handle(runner: &Arc<Runner>, text: &str, out: &mpsc::Sender<Value>, connection: &CancellationToken) {
    let frame = match serde_json::from_str::<ServerFrame>(text) {
        Ok(frame) => frame,
        Err(_) => {
            let id = protocol::id_of(text);
            let _ = out.try_send(protocol::error(id.as_deref(), "bad_request", "Malformed frame"));
            return;
        }
    };
    let out = out.clone();
    match frame {
        ServerFrame::Ping { id } => {
            let _ = out.try_send(protocol::pong(id.as_deref()));
        }
        ServerFrame::Unknown => {}
        ServerFrame::LoginStart { id, vendor } => {
            let Some(vendor) = vendor_for(runner, &id, &vendor, &out) else { return };
            let runner = runner.clone();
            tokio::spawn(async move {
                let frame = login_start(&runner.cfg, &id, vendor).await;
                let _ = out.send(frame).await;
            });
        }
        ServerFrame::LoginSubmit { id, vendor, value } => {
            let Some(vendor) = vendor_for(runner, &id, &vendor, &out) else { return };
            tokio::spawn(async move {
                let status = login_submit(vendor, &value).await;
                let _ = out.send(json!({"type": "submit_result", "id": id, "status": status})).await;
            });
        }
        ServerFrame::LoginStatus { id, vendor } => {
            let Some(vendor) = vendor_for(runner, &id, &vendor, &out) else { return };
            let runner = runner.clone();
            tokio::spawn(async move {
                let status = login_status(&runner.cfg, vendor).await;
                let _ = out.send(json!({"type": "login_status_result", "id": id, "status": status})).await;
            });
        }
        ServerFrame::Logout { id, vendor } => {
            let Some(vendor) = vendor_for(runner, &id, &vendor, &out) else { return };
            let runner = runner.clone();
            tokio::spawn(async move {
                let deleted = logout(&runner.cfg, vendor).await;
                let _ = out.send(json!({"type": "logout_done", "id": id, "home_deleted": deleted})).await;
            });
        }
        ServerFrame::Cancel { id } => {
            if let Some((running, token)) = runner.turn.lock().unwrap().as_ref() {
                if *running == id {
                    token.cancel();
                }
            }
        }
        ServerFrame::Turn { id, vendor, model, system, messages } => {
            let Some(vendor) = vendor_for(runner, &id, &vendor, &out) else { return };
            let (system, history) = match normalize(system, messages) {
                Ok(parts) => parts,
                Err(message) => {
                    let _ = out.try_send(protocol::error(Some(&id), "bad_request", &message));
                    return;
                }
            };
            let cancel = connection.child_token();
            {
                let mut slot = runner.turn.lock().unwrap();
                if slot.is_some() {
                    let _ = out.try_send(protocol::error(Some(&id), "bad_request", "A turn is already running"));
                    return;
                }
                *slot = Some((id.clone(), cancel.clone()));
            }
            let model = model.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| vendor.default_model().to_string());
            let runner = runner.clone();
            tokio::spawn(async move {
                info!(vendor = vendor.id(), "turn started");
                let request = process::TurnRequest { id: id.clone(), vendor, model, system, history };
                process::run_turn(&runner.cfg, request, out, cancel).await;
                let mut slot = runner.turn.lock().unwrap();
                if slot.as_ref().is_some_and(|(running, _)| *running == id) {
                    *slot = None;
                }
                info!(vendor = vendor.id(), "turn ended");
            });
        }
    }
}

/// A message's text: a string, or the `text` parts of an OpenAI-style array.
fn message_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => p.get("text").and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.join("")),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

/// Validates a turn's messages: system text joins the system prompt, the rest
/// is user/assistant history ending with the new user message. Tool messages
/// and non-text parts are refused (chat only).
fn normalize(system: Option<String>, messages: Vec<protocol::WireMessage>) -> Result<(String, Vec<Msg>), String> {
    let mut system_parts: Vec<String> = system.into_iter().filter(|s| !s.trim().is_empty()).collect();
    let mut history = Vec::new();
    for m in messages {
        let text = message_text(&m.content).ok_or("Only text messages are supported")?;
        match m.role.as_str() {
            "system" | "developer" => {
                if !text.trim().is_empty() {
                    system_parts.push(text);
                }
            }
            "user" => history.push(Msg { user: true, content: text }),
            "assistant" => history.push(Msg { user: false, content: text }),
            _ => return Err("Only user and assistant messages are supported".into()),
        }
    }
    match history.last() {
        Some(last) if last.user && !last.content.trim().is_empty() => {}
        _ => return Err("The last message must be a user message".into()),
    }
    let system = if system_parts.is_empty() { vendors::DEFAULT_SYSTEM.to_string() } else { system_parts.join("\n\n") };
    Ok((system, history))
}

fn login_key(vendor: Vendor) -> String {
    format!("brain-runner:{}", vendor.id())
}

async fn login_start(cfg: &Config, id: &str, vendor: Vendor) -> Value {
    let Some(binary) = vendors::binary(cfg, vendor) else {
        return protocol::error(Some(id), "cli_failed", &format!("{} is not installed", vendor.binary_name()));
    };
    let home = vendors::home(cfg, vendor);
    if let Err(e) = std::fs::create_dir_all(&home) {
        return protocol::error(Some(id), "cli_failed", &format!("could not make the {} home: {e}", vendor.id()));
    }
    let env = vendors::env(cfg, vendor);
    let env_refs: Vec<(&str, &std::ffi::OsStr)> = env.iter().map(|(k, v)| (k.as_str(), v.as_os_str())).collect();
    let key = login_key(vendor);
    let started = chrono::Utc::now();
    let view = match crate::cli_tools::start_login_with_env(&key, &binary, vendor.login_argv(), None, &env_refs).await {
        Ok(view) => view,
        Err(e) => return protocol::error(Some(id), "cli_failed", &format!("could not start the sign-in: {e}")),
    };
    // start_login_with_env waits 3 s for the link; give a slow CLI up to 15 s.
    let mut view = view;
    for _ in 0..24 {
        if view.get("url").and_then(Value::as_str).is_some() || view.get("exited") == Some(&Value::Bool(true)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some((latest, _, _)) = crate::cli_tools::login_view(&key).await {
            view = latest;
        }
    }
    let url = view.get("url").and_then(Value::as_str);
    if url.is_none() {
        let reason = view.get("error").and_then(Value::as_str).unwrap_or("the CLI printed no sign-in link");
        return protocol::error(Some(id), "cli_failed", reason);
    }
    let (url, code, url_with_code) = vendors::login_fields(vendor, url, view.get("code").and_then(Value::as_str));
    let expires_at = (started + chrono::Duration::from_std(LOGIN_EXPIRES).unwrap_or_default())
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    json!({
        "type": "login_view", "id": id, "kind": vendor.login_kind(),
        "code": code, "url": url, "url_with_code": url_with_code, "expires_at": expires_at,
    })
}

/// Waits briefly for a login process to finish after a pasted code.
async fn login_submit(vendor: Vendor, value: &str) -> &'static str {
    let key = login_key(vendor);
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 || value.contains(['\n', '\r']) {
        return "failed";
    }
    if crate::cli_tools::submit_code(&key, value).await.is_err() {
        return "expired";
    }
    for _ in 0..40 {
        match crate::cli_tools::login_view(&key).await {
            Some((_, true, true)) => return "done",
            Some((_, true, false)) => return "failed",
            Some(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            None => return "expired",
        }
    }
    "pending"
}

async fn login_status(cfg: &Config, vendor: Vendor) -> &'static str {
    let live = crate::cli_tools::login_view(&login_key(vendor)).await;
    if let Some((_, false, _)) = live {
        return "pending";
    }
    match process::sign_in_state(cfg, vendor).await {
        Ok(SignIn::In { .. }) => "done",
        Ok(SignIn::Out) => match live {
            Some((view, true, _)) if view.get("error").and_then(Value::as_str) == Some("sign-in was stopped") => "expired",
            Some((_, true, _)) => "failed",
            _ => "expired",
        },
        Err(_) => "failed",
    }
}

/// CLI logout (best effort), then delete that vendor's home. Kimi has no
/// logout command, so its credentials and oauth are deleted directly.
async fn logout(cfg: &Config, vendor: Vendor) -> bool {
    // A sign-in still waiting would write its credentials back after the
    // home is deleted, so stop it first.
    crate::cli_tools::stop_login(&login_key(vendor)).await;
    let home = vendors::home(cfg, vendor);
    match vendor.logout_argv() {
        Some(args) => {
            // 20 s: cloud-api waits 30 s for the logout_done reply.
            if let Err(error) = process::run_quick(cfg, vendor, args, Duration::from_secs(20)).await {
                warn!(vendor = vendor.id(), %error, "CLI logout failed; deleting the home anyway");
            }
        }
        None => {
            for name in ["credentials", "oauth"] {
                let path = home.join(name);
                let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
            }
        }
    }
    if !vendors::home_is_owned(cfg, vendor) {
        warn!(vendor = vendor.id(), "vendor home is a development override; not deleting it");
        return false;
    }
    if !home.exists() {
        return true;
    }
    if !vendors::path_is_inside(&home, &cfg.home) {
        warn!(vendor = vendor.id(), "vendor home resolves outside the runner home; not deleting it");
        return false;
    }
    match std::fs::remove_dir_all(&home) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            warn!(vendor = vendor.id(), %error, "could not delete the vendor home");
            false
        }
    }
}
