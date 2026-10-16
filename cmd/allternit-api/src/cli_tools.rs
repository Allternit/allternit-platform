//! One-click install and remote-friendly sign-in for CLI brains (Claude Code,
//! Codex, Grok, ...), on whichever computer this runtime runs: the user's
//! Mac, their VPS, or an Allternit cloud computer reached through the relay.
//!
//! - **Install** (`POST /api/v1/cli-tools/:id/install`) runs the Factory's
//!   version-pinned, licence-gated installer (`allternit-factory pane harness
//!   install <tool>`, manifest `factory/pane/src/ao/harness/harness.json`) and
//!   streams its output as SSE. Tools land in the harness `bin/`, which
//!   `main` puts first on this process's PATH (and gizzi does the same), so a
//!   fresh install resolves without a restart. Machines without npm get a
//!   private, checksum-verified Node from the installer.
//! - **Sign in** keeps the CLI's own login process running with its output
//!   captured, so the sign-in link, any device code, and a "paste the code"
//!   prompt reach the UI instead of a browser on a machine nobody is looking
//!   at. `POST /providers/:id/connect/code` writes a pasted code to its stdin.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::auth::get_user;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/cli-tools/:id/install", post(install_cli_tool))
}

// ─── Manifest ────────────────────────────────────────────────────────────────

static MANIFEST: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(include_str!("../../../factory/pane/src/ao/harness/harness.json"))
        .unwrap_or(Value::Null)
});

/// What the install manifest says about one CLI.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolInfo {
    pub key: String,
    pub license: String,
    pub pin: Option<String>,
    pub requires_terms: bool,
    pub terms_url: Option<String>,
    pub available: bool,
}

fn harness_key(provider_id: &str) -> String {
    match provider_id {
        "antigravity" => "agy".to_string(),
        other => other.strip_suffix("-cli").unwrap_or(other).to_string(),
    }
}

/// The manifest entry for a provider, when the manifest installs the same
/// binary this runtime runs for it (`verifyCmd`'s program == `binary`).
pub(crate) fn tool_info(provider_id: &str, binary: &str) -> Option<ToolInfo> {
    tool_info_in(&MANIFEST, provider_id, binary)
}

fn tool_info_in(manifest: &Value, provider_id: &str, binary: &str) -> Option<ToolInfo> {
    let key = harness_key(provider_id);
    let tool = manifest.get("tools")?.get(&key)?;
    let install = tool.get("install")?;
    let method = install.get("method")?.as_str()?;
    if method == "unsupported" {
        return None;
    }
    let verify_bin = install
        .get("verifyCmd")
        .and_then(Value::as_str)
        .and_then(|cmd| cmd.split_whitespace().next())
        .unwrap_or(&key);
    if verify_bin != binary {
        return None;
    }
    let license = tool
        .get("license")
        .and_then(Value::as_str)
        .unwrap_or("undeclared")
        .to_string();
    let requires_terms = !matches!(license.as_str(), "apache" | "mit" | "bsd");
    let terms_url = tool
        .get("_licenseNote")
        .and_then(Value::as_str)
        .and_then(|note| URL_RE.find(note))
        .map(|m| m.as_str().trim_end_matches([')', '.', ',', ';']).to_string());
    let available = !cfg!(windows)
        && match method {
            "venv-pip" => crate::provider_routes::command_on_path("python3").is_some(),
            _ => true,
        };
    Some(ToolInfo {
        key,
        license,
        pin: install.get("pinnedVersion").and_then(Value::as_str).map(str::to_string),
        requires_terms,
        terms_url,
        available,
    })
}

/// The `install` field of a provider auth-status row.
pub(crate) fn install_json(provider_id: &str, binary: &str) -> Value {
    match tool_info(provider_id, binary) {
        Some(info) => json!({
            "available": info.available,
            "requiresTerms": info.requires_terms,
            "license": info.license,
            "termsUrl": info.terms_url,
            "pin": info.pin,
        }),
        None => Value::Null,
    }
}

// ─── Install ─────────────────────────────────────────────────────────────────

static INSTALLING: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallBody {
    #[serde(default)]
    accept_terms: bool,
}

fn sse(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

async fn install_cli_tool(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<InstallBody>>,
) -> Response {
    if get_user(&headers).is_none() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let accept_terms = body.map(|Json(b)| b.accept_terms).unwrap_or(false);
    let Some(binary) = crate::provider_routes::cli_binary_for(&id) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "unknown_tool"}))).into_response();
    };
    let Some(info) = tool_info(&id, binary) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "unknown_tool"}))).into_response();
    };
    if !info.available {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "install_unavailable", "message": "One-click install isn't available on this computer"})),
        )
            .into_response();
    }
    if info.requires_terms && !accept_terms {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "terms_required", "license": info.license, "termsUrl": info.terms_url})),
        )
            .into_response();
    }
    if !INSTALLING.lock().unwrap().insert(info.key.clone()) {
        return (StatusCode::CONFLICT, Json(json!({"error": "install_in_progress"}))).into_response();
    }

    let mut command = tokio::process::Command::new(crate::terminal_routes::factory_bin());
    command
        .args(["pane", "harness", "install", &info.key])
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if info.requires_terms {
        command.args(["--accept-terms", &info.key]);
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);
    let key = info.key.clone();
    let binary = binary.to_string();
    match command.spawn() {
        Err(error) => {
            INSTALLING.lock().unwrap().remove(&key);
            let _ = tx.try_send(sse("done", json!({"ok": false, "error": format!("could not start the installer: {error}")})));
        }
        Ok(mut child) => {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            tokio::spawn(async move {
                let readers = [stdout.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
                    stderr.map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>)];
                let mut tasks = Vec::new();
                for reader in readers.into_iter().flatten() {
                    let tx = tx.clone();
                    tasks.push(tokio::spawn(async move {
                        use tokio::io::AsyncBufReadExt;
                        let mut lines = tokio::io::BufReader::new(reader).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let line = strip_ansi(&line);
                            if !line.trim().is_empty() {
                                let _ = tx.send(sse("log", json!({"line": line}))).await;
                            }
                        }
                    }));
                }
                // npm can be silent for minutes; keep proxies and the user informed.
                let heartbeat_tx = tx.clone();
                let heartbeat = tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_secs(15));
                    tick.tick().await;
                    loop {
                        tick.tick().await;
                        if heartbeat_tx.send(sse("log", json!({"line": "still installing…"}))).await.is_err() {
                            break;
                        }
                    }
                });
                let status = tokio::time::timeout(Duration::from_secs(15 * 60), child.wait()).await;
                heartbeat.abort();
                for task in tasks {
                    let _ = task.await;
                }
                let done = match status {
                    Ok(Ok(status)) if status.success() => match resolve_binary(&binary) {
                        Some(path) => {
                            reload_gizzi().await;
                            json!({"ok": true, "path": path.display().to_string()})
                        }
                        None => json!({"ok": false, "error": format!("the installer finished but {binary} wasn't found")}),
                    },
                    Ok(Ok(status)) => json!({"ok": false, "error": format!("the installer failed ({status})")}),
                    Ok(Err(error)) => json!({"ok": false, "error": error.to_string()}),
                    Err(_) => {
                        let _ = child.kill().await;
                        json!({"ok": false, "error": "the install took longer than 15 minutes and was stopped"})
                    }
                };
                INSTALLING.lock().unwrap().remove(&key);
                let _ = tx.send(sse("done", done)).await;
            });
        }
    }

    let stream = tokio_stream_from(rx);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .unwrap_or_default()
}

fn tokio_stream_from(
    mut rx: tokio::sync::mpsc::Receiver<String>,
) -> impl futures::Stream<Item = Result<String, std::convert::Infallible>> {
    async_stream::stream! {
        while let Some(chunk) = rx.recv().await {
            yield Ok(chunk);
        }
    }
}

/// A CLI on PATH (which includes the harness `bin/`, see `main`), or in the
/// harness `bin/` even if PATH was changed after startup.
pub(crate) fn resolve_binary(binary: &str) -> Option<PathBuf> {
    crate::provider_routes::command_on_path(binary).or_else(|| {
        let candidate = allternit_factory_engine::registry::harness_bin_dir().join(binary);
        candidate.is_file().then_some(candidate)
    })
}

/// gizzi lists providers once per instance; reload it so a CLI that was just
/// installed or signed in shows up in the model picker.
pub(crate) async fn reload_gizzi() {
    let url = format!("{}/instance/dispose", crate::agent_session_routes::gizzi_base());
    if let Err(error) = reqwest::Client::new().post(url).timeout(Duration::from_secs(10)).send().await {
        tracing::warn!(%error, "could not reload gizzi after a CLI install or sign-in");
    }
}

// ─── Sign in ─────────────────────────────────────────────────────────────────

static URL_RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r#"https://[^\s"'<>]+"#).unwrap());
static CODE_RE: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\b[A-Z0-9]{4,5}-[A-Z0-9]{4,5}\b").unwrap());
static ANSI_RE: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07").unwrap());

fn strip_ansi(text: &str) -> String {
    ANSI_RE.replace_all(text, "").into_owned()
}

/// What a login process has told us so far.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct LoginOutput {
    pub url: Option<String>,
    pub code: Option<String>,
    pub needs_code: bool,
}

impl LoginOutput {
    /// Fold one chunk of (ANSI-stripped) output in. The first link and the
    /// first device-style code win; "paste code" anywhere means the CLI waits
    /// for the user to paste the code the sign-in page shows them.
    fn absorb(&mut self, text: &str) {
        if self.url.is_none() {
            self.url = URL_RE.find(text).map(|m| m.as_str().to_string());
        }
        if self.code.is_none() {
            // Codes printed on their own (Codex, Grok), not ones that are only
            // part of a URL's query (that would just repeat the link).
            let without_urls = URL_RE.replace_all(text, " ");
            self.code = CODE_RE.find(&without_urls).map(|m| m.as_str().to_string());
        }
        if text.to_lowercase().contains("paste code") {
            self.needs_code = true;
        }
    }
}

struct LoginSession {
    output: LoginOutput,
    hint: Option<String>,
    tail: VecDeque<String>,
    exited: bool,
    success: bool,
    error: Option<String>,
    stdin: Option<tokio::process::ChildStdin>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
    reloaded: bool,
}

type Shared = Arc<tokio::sync::Mutex<LoginSession>>;
static LOGINS: Lazy<Mutex<HashMap<String, Shared>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn session(key: &str) -> Option<Shared> {
    LOGINS.lock().unwrap().get(key).cloned()
}

/// Start (or restart) a CLI's own sign-in for `key`, keeping its output.
/// Waits up to 3 s for the link so the first response usually carries it.
pub(crate) async fn start_login(
    key: &str,
    binary: &std::path::Path,
    argv: &[&str],
    hint: Option<String>,
) -> Result<Value, String> {
    if let Some(old) = LOGINS.lock().unwrap().remove(key) {
        if let Ok(mut old) = old.try_lock() {
            if let Some(kill) = old.kill.take() {
                let _ = kill.send(());
            }
        }
    }
    let mut child = tokio::process::Command::new(binary)
        .args(argv)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    let shared: Shared = Arc::new(tokio::sync::Mutex::new(LoginSession {
        output: LoginOutput::default(),
        hint,
        tail: VecDeque::new(),
        exited: false,
        success: false,
        error: None,
        stdin: child.stdin.take(),
        kill: Some(kill_tx),
        reloaded: false,
    }));
    LOGINS.lock().unwrap().insert(key.to_string(), shared.clone());

    let mut readers = Vec::new();
    for reader in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
    ]
    .into_iter()
    .flatten()
    {
        let shared = shared.clone();
        readers.push(tokio::spawn(async move {
            // Chunks, not lines: Claude's "Paste code here >" prompt has no newline.
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            let mut pending = String::new();
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                let text = strip_ansi(&pending);
                let mut s = shared.lock().await;
                s.output.absorb(&text);
                let mut parts: Vec<&str> = text.split(['\n', '\r']).collect();
                let partial = parts.pop().unwrap_or_default().to_string();
                for line in parts.into_iter().filter(|l| !l.trim().is_empty()) {
                    s.tail.push_back(line.to_string());
                    if s.tail.len() > 40 {
                        s.tail.pop_front();
                    }
                }
                pending = partial;
            }
        }));
    }

    let waiter = shared.clone();
    tokio::spawn(async move {
        let outcome = tokio::select! {
            status = child.wait() => Some(status),
            _ = kill_rx => { let _ = child.kill().await; None }
            _ = tokio::time::sleep(Duration::from_secs(15 * 60)) => { let _ = child.kill().await; None }
        };
        for reader in readers {
            let _ = reader.await;
        }
        let mut s = waiter.lock().await;
        s.exited = true;
        s.stdin = None;
        match outcome {
            Some(Ok(status)) if status.success() => s.success = true,
            Some(Ok(status)) => {
                s.error = Some(s.tail.back().cloned().unwrap_or_else(|| format!("sign-in exited ({status})")))
            }
            Some(Err(error)) => s.error = Some(error.to_string()),
            None => s.error = Some("sign-in was stopped".to_string()),
        }
    });

    for _ in 0..30 {
        if shared.lock().await.output.url.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let view = login_json(&*shared.lock().await);
    Ok(view)
}

fn login_json(s: &LoginSession) -> Value {
    json!({
        "url": s.output.url,
        "code": s.output.code,
        "needsCode": s.output.needs_code && !s.exited,
        "exited": s.exited,
        "error": s.error,
        "hint": s.hint,
    })
}

/// The live sign-in for `key`, if one was started.
pub(crate) async fn login_view(key: &str) -> Option<(Value, bool, bool)> {
    let shared = session(key)?;
    let s = shared.lock().await;
    Some((login_json(&s), s.exited, s.success))
}

/// Reload gizzi once when a started sign-in completes.
pub(crate) async fn note_signed_in(key: &str) {
    let Some(shared) = session(key) else { return };
    let first = {
        let mut s = shared.lock().await;
        !std::mem::replace(&mut s.reloaded, true)
    };
    if first {
        reload_gizzi().await;
    }
}

/// Paste a code into a waiting sign-in (Claude).
pub(crate) async fn submit_code(key: &str, code: &str) -> Result<(), &'static str> {
    let shared = session(key).ok_or("no_login_waiting")?;
    let mut s = shared.lock().await;
    let stdin = s.stdin.as_mut().ok_or("no_login_waiting")?;
    let line = format!("{}\n", code.trim());
    stdin.write_all(line.as_bytes()).await.map_err(|_| "no_login_waiting")?;
    stdin.flush().await.map_err(|_| "no_login_waiting")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_output_gives_link_and_asks_for_the_code() {
        let mut out = LoginOutput::default();
        out.absorb("Opening browser to sign in…\nIf the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&response_type=code\nPaste code here if prompted > ");
        assert_eq!(
            out.url.as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&response_type=code")
        );
        assert!(out.needs_code);
        assert_eq!(out.code, None);
    }

    #[test]
    fn codex_device_output_gives_link_and_code() {
        let mut out = LoginOutput::default();
        out.absorb(&strip_ansi("Follow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   \x1b[94mhttps://auth.openai.com/codex/device\x1b[0m\n\n2. Enter this one-time code \x1b[90m(expires in 15 minutes)\x1b[0m\n   \x1b[94mZWBG-JYPGA\x1b[0m\n"));
        assert_eq!(out.url.as_deref(), Some("https://auth.openai.com/codex/device"));
        assert_eq!(out.code.as_deref(), Some("ZWBG-JYPGA"));
        assert!(!out.needs_code);
    }

    #[test]
    fn grok_device_output_gives_link_and_code() {
        let mut out = LoginOutput::default();
        out.absorb("To sign in, open this URL in your browser:\n\n  https://accounts.x.ai/oauth2/device?user_code=4782-CGRP\n\nConfirm this code in your browser:\n\n  4782-CGRP\n");
        assert_eq!(out.url.as_deref(), Some("https://accounts.x.ai/oauth2/device?user_code=4782-CGRP"));
        assert_eq!(out.code.as_deref(), Some("4782-CGRP"));
    }

    #[test]
    fn output_split_across_chunks_still_parses() {
        let mut out = LoginOutput::default();
        out.absorb("To sign in, open");
        out.absorb("To sign in, open this URL:\n  https://accounts.x.ai/oauth2/device?user_code=4782-CGRP\n");
        assert!(out.url.is_some());
    }

    #[test]
    fn manifest_maps_providers_to_tools_and_gates_proprietary_ones() {
        let claude = tool_info("claude-cli", "claude").expect("claude in manifest");
        assert_eq!(claude.key, "claude");
        assert!(claude.requires_terms);
        assert!(claude.terms_url.as_deref().unwrap_or("").starts_with("https://www.anthropic.com/"));
        let codex = tool_info("codex", "codex").expect("codex in manifest");
        assert!(!codex.requires_terms);
        assert!(tool_info("grok", "grok").is_some());
        // A provider whose binary differs from the manifest's is not offered.
        assert!(tool_info("claude-cli", "not-claude").is_none());
        assert!(tool_info("unknown-tool", "x").is_none());
    }
}
