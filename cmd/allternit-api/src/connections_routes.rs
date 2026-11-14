//! Runtime side of the connect kit (`CONTRACT-connections-2026-11.md` sections 3-5).
//!
//! `GET /api/v1/connections` answers for the things that live on THIS computer:
//! the coding CLIs, ChatGPT through Codex, the local (Ollama) brain, and keys
//! saved on the device. Each row carries `source: "runtime:<id>"`. The client
//! merges these with cloud-api's rows.
//!
//! The CLI sign-ins reuse `cli_tools::start_login_with_env` and the catalog's
//! `login_remote` argv, so they are the same sign-ins Settings runs, shaped as
//! the contract's `login` object. Nothing here calls `require_compute`.
//!
//! `chatgpt` (contract §9.9): Sign in with ChatGPT on this computer is owned
//! by Gizzi Code. This row drives it through the gizzi binary
//! (`gizzi connect chatgpt --json`, `gizzi connect status chatgpt --json`,
//! `gizzi connect disconnect chatgpt --json`) with login kind
//! `browser_redirect`: the client opens the authorize URL, the sign-in
//! completes on 127.0.0.1 by itself, and `/login` polls. The disclosure
//! comes first: `connect` without the current `accept_disclosure` answers
//! `409 disclosure_required` with gizzi's text, and gizzi records the
//! acceptance on this computer. Codex signed in here still turns the row on
//! when the device sign-in is off (or gizzi is missing).
//!
//! Path note: `/api/v1/connections` is new. `design_connector_routes.rs` only
//! serves `/design/composio/connections`, and `ssh_routes.rs` serves
//! `/ssh-connections`, so there is no overlap. The smoke-boot in the PR proves it.

use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::get_user;

/// A runtime row id and the catalog CLI behind it.
struct CliRow {
    id: &'static str,
    group: &'static str,
    /// Catalog tool id (`cli_catalog::find`).
    tool: &'static str,
}

const CLI_ROWS: &[CliRow] = &[
    CliRow { id: "claude-code", group: "tools", tool: "claude-cli" },
    CliRow { id: "codex", group: "tools", tool: "codex-cli" },
    CliRow { id: "gemini-cli", group: "tools", tool: "gemini-cli" },
    CliRow { id: "cursor", group: "tools", tool: "cursor-agent" },
    CliRow { id: "grok-cli", group: "tools", tool: "grok" },
    CliRow { id: "kimi-cli", group: "tools", tool: "kimi-cli" },
    CliRow { id: "chatgpt", group: "brains", tool: "codex-cli" },
];

/// Gizzi provider id of a key saved on this device, and the kit row it fills.
const DEVICE_KEYS: &[(&str, &str, &str)] = &[
    ("openai", "openai-key", "OpenAI"),
    ("anthropic", "anthropic-key", "Claude"),
    ("google", "google-key", "Google"),
    ("xai", "xai-key", "Grok"),
    ("togetherai", "together-key", "Together"),
    ("fireworks-ai", "fireworks-key", "Fireworks"),
    ("deepinfra", "deepinfra-key", "DeepInfra"),
    ("groq", "groq-key", "Groq"),
    ("deepseek", "deepseek-key", "DeepSeek"),
    ("moonshotai", "kimi-key", "Kimi"),
    ("openrouter", "openrouter-key", "OpenRouter"),
];

/// No handler reads `AppState`, so the router fits any state type.
pub fn connections_router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/connections", get(list_connections))
        .route("/connections/:id", delete(disconnect))
        .route("/connections/:id/connect", post(connect))
        .route("/connections/:id/submit", post(submit))
        .route("/connections/:id/login", get(login_status))
}

fn err(status: StatusCode, code: &str, text: &str) -> Response {
    (status, Json(json!({ "error": code, "message": text }))).into_response()
}

fn source() -> String {
    use crate::relay_auth::RelaySecret;
    let id = crate::relay_auth::EnvOrFileRelaySecret::from_process_env()
        .runtime_id()
        .unwrap_or_else(|| "local".to_string());
    format!("runtime:{id}")
}

// ─── Pure helpers (unit tested) ───────────────────────────────────────────────

/// The plan name in Codex's `auth.json` (`tokens.id_token` is a JWT whose
/// `https://api.openai.com/auth` claim holds `chatgpt_plan_type`).
pub(crate) fn codex_plan_from_auth(auth_json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(auth_json).ok()?;
    let jwt = v.pointer("/tokens/id_token")?.as_str()?;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let plan = claims.pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type")?.as_str()?;
    let plan = plan.trim();
    if plan.is_empty() {
        return None;
    }
    let mut chars = plan.chars();
    let first = chars.next()?.to_uppercase().collect::<String>();
    Some(format!("ChatGPT {first}{}", chars.as_str()))
}

/// What the CLI printed, shaped as the contract's `login` object.
pub(crate) fn login_object(
    tool: &str,
    url: Option<&str>,
    code: Option<&str>,
    needs_code: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Value {
    let kind = match (tool, code, needs_code) {
        ("claude-cli", ..) => "link_paste",
        (_, Some(_), _) => "device_code",
        (_, None, true) => "link_paste",
        _ => "oauth",
    };
    let url_with_code = url.filter(|u| u.contains("user_code="));
    // Codex's code lasts 15 minutes; the CLI itself is stopped at 15 minutes.
    let expires_at = (now + chrono::Duration::minutes(15)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    json!({
        "kind": kind,
        "code": code,
        "url": url,
        "url_with_code": url_with_code,
        "expires_at": expires_at,
        "poll": true,
    })
}

/// `GET .../login` status from the CLI's own state: signed in, whether the
/// sign-in process has ended, whether it ended well, and its last error.
pub(crate) fn login_status_word(signed_in: bool, session: Option<(bool, bool, Option<&str>)>) -> &'static str {
    if signed_in {
        return "done";
    }
    match session {
        // No sign-in is running here (never started, or the server restarted).
        None => "expired",
        Some((false, _, _)) => "pending",
        Some((true, true, _)) => "done",
        // cli_tools kills the CLI at 15 minutes or on a new start.
        Some((true, false, Some("sign-in was stopped"))) => "expired",
        Some((true, false, _)) => "failed",
    }
}

/// Logout argv for the CLIs that have one.
pub(crate) fn logout_argv(tool: &str) -> Option<&'static [&'static str]> {
    match tool {
        "claude-cli" => Some(&["auth", "logout"]),
        "codex-cli" | "grok" | "kimi-cli" | "cursor-agent" => Some(&["logout"]),
        _ => None,
    }
}

// ─── ChatGPT direct sign-in (through gizzi, §9.9) ─────────────────────────────

/// Disclosure version (§9.8); gizzi owns the text and the stored acceptance.
pub(crate) const CHATGPT_DISCLOSURE_VERSION: &str = "2026-11-14.1";

/// The last JSON object gizzi printed (it prints JSON lines).
pub(crate) fn last_json_line(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .rev()
        .filter(|l| l.trim_start().starts_with('{'))
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
}

/// Run gizzi with these args and read its JSON answer.
async fn gizzi_json(bin: &std::path::Path, args: &[&str], secs: u64) -> Option<Value> {
    let run = tokio::process::Command::new(bin)
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(secs), run).await {
        Ok(Ok(out)) => last_json_line(&String::from_utf8_lossy(&out.stdout)),
        _ => None,
    }
}

/// `gizzi connect status chatgpt --json` → the device sign-in state.
async fn chatgpt_device_status(bin: &std::path::Path) -> Option<Value> {
    gizzi_json(bin, &["connect", "status", "chatgpt", "--json"], 15).await
}

/// The `chatgpt` row from gizzi's status (`state`, `found`, `models`, `disclosure`).
pub(crate) fn chatgpt_row_from_status(status: &Value, src: &str) -> Value {
    let state = match status.get("state").and_then(Value::as_str) {
        Some(s @ ("off" | "connecting" | "on" | "needs")) => s,
        _ => "off",
    };
    let found = (state == "on").then(|| status.get("found").and_then(Value::as_str).unwrap_or("Signed in on this computer").to_string());
    let reason = (state == "needs").then(|| {
        let code = status.get("reason_code").and_then(Value::as_str).unwrap_or("could_not_start");
        let text = if code == "login_expired" { "Sign-in expired" } else { "Could not start. Try again" };
        (code.to_string(), text)
    });
    let models: Vec<Value> = status.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
    json!({
        "id": "chatgpt",
        "group": "brains",
        "state": state,
        "found": found,
        "reason": reason.as_ref().map(|(_, t)| *t),
        "reason_code": reason.as_ref().map(|(c, _)| c.clone()),
        "runs_on": "computer",
        "available": true,
        "unavailable_reason": null,
        "fallbacks": [],
        "terms": "pending",
        "saved": "device",
        "models": models,
        "person_only": true,
        "disclosure": status.get("disclosure").cloned().unwrap_or_else(|| json!({ "version": CHATGPT_DISCLOSURE_VERSION, "accepted_at": null })),
        "source": src,
    })
}

/// `/login` status word from gizzi's state.
pub(crate) fn chatgpt_login_word(status: &Value) -> &'static str {
    match status.get("state").and_then(Value::as_str) {
        Some("on") => "done",
        Some("connecting") => "pending",
        Some("needs") if status.get("reason_code").and_then(Value::as_str) == Some("login_expired") => "expired",
        Some("needs") => "failed",
        _ => "expired",
    }
}

fn disclosure_required(disclosure: Value) -> Response {
    (StatusCode::CONFLICT, Json(json!({ "error": { "code": "disclosure_required", "disclosure": disclosure } }))).into_response()
}

/// The running `gizzi connect chatgpt --json` (one at a time; a new start replaces it).
static CHATGPT_SIGN_IN: once_cell::sync::Lazy<tokio::sync::Mutex<Option<tokio::process::Child>>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(None));

/// `POST /connections/chatgpt/connect` through gizzi: the disclosure step,
/// then the sign-in started and its authorize URL as a `browser_redirect` login.
pub(crate) async fn connect_chatgpt_with(bin: &std::path::Path, accept: Option<&str>) -> Response {
    if let Some(status) = chatgpt_device_status(bin).await {
        if status.get("state").and_then(Value::as_str) == Some("on") {
            return Json(json!({ "state": "on" })).into_response();
        }
    }
    if accept != Some(CHATGPT_DISCLOSURE_VERSION) {
        // gizzi answers `needs` + `disclosure_required` (and exits) until the version is accepted.
        let already = chatgpt_device_status(bin)
            .await
            .and_then(|s| s.pointer("/disclosure/accepted_at").and_then(Value::as_str).map(str::to_string))
            .is_some();
        if !already {
            let answer = gizzi_json(bin, &["connect", "chatgpt", "--json"], 20).await;
            return match answer.and_then(|a| a.get("disclosure").cloned()) {
                Some(d) => disclosure_required(d),
                None => err(StatusCode::INTERNAL_SERVER_ERROR, "could_not_start", "Could not start. Try again"),
            };
        }
    }
    let mut args = vec!["connect", "chatgpt", "--json", "--no-open"];
    if let Some(v) = accept {
        args.push("--accept-disclosure");
        args.push(v);
    }
    let mut guard = CHATGPT_SIGN_IN.lock().await;
    if let Some(mut old) = guard.take() {
        let _ = old.start_kill();
    }
    let spawned = tokio::process::Command::new(bin)
        .args(&args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("chatgpt sign-in did not start: {e}");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "could_not_start", "Could not start. Try again");
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "could_not_start", "Could not start. Try again");
    };
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let first = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(v) = last_json_line(&line) {
                return Some(v);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    // Keep draining gizzi's later lines so it never blocks on a full pipe; `/login` reads status instead.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    let Some(first) = first else {
        let _ = child.start_kill();
        return err(StatusCode::INTERNAL_SERVER_ERROR, "could_not_start", "Could not start. Try again");
    };
    match first.get("state").and_then(Value::as_str) {
        Some("connecting") => {
            *guard = Some(child);
            let url = first.get("authorize_url").cloned().unwrap_or(Value::Null);
            let expires_at = first.get("expires_at").cloned().unwrap_or(Value::Null);
            Json(json!({ "login": { "kind": "browser_redirect", "code": null, "url": url, "url_with_code": null, "expires_at": expires_at, "poll": true } }))
                .into_response()
        }
        Some("needs") if first.get("reason_code").and_then(Value::as_str) == Some("disclosure_required") => {
            disclosure_required(first.get("disclosure").cloned().unwrap_or(Value::Null))
        }
        Some("on") => Json(json!({ "state": "on" })).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "could_not_start", "Could not start. Try again"),
    }
}

fn find_row(id: &str) -> Option<&'static CliRow> {
    CLI_ROWS.iter().find(|r| r.id == id)
}

fn row_json(
    r: &CliRow,
    state: &str,
    found: Option<String>,
    reason: Option<(&str, &str)>,
    available: bool,
    unavailable_reason: Option<&str>,
    models: Vec<String>,
    src: &str,
) -> Value {
    json!({
        "id": r.id,
        "group": r.group,
        "state": state,
        "found": found,
        "reason": reason.map(|(_, t)| t),
        "reason_code": reason.map(|(c, _)| c),
        "runs_on": "computer",
        "available": available,
        "unavailable_reason": unavailable_reason,
        "fallbacks": [],
        "terms": "cleared",
        "saved": "device",
        "models": models,
        "source": src,
    })
}

// ─── Row building ─────────────────────────────────────────────────────────────

async fn cli_row(r: &'static CliRow, src: String) -> Value {
    // ChatGPT signed in on this computer (gizzi) wins; Codex answers otherwise.
    if r.id == "chatgpt" {
        if let Some(bin) = crate::cloud_worker::gizzi_bin() {
            if let Some(status) = chatgpt_device_status(&bin).await {
                let codex = codex_chatgpt_row(r, src.clone()).await;
                if status.get("state").and_then(Value::as_str) != Some("off") || codex["state"] != "on" {
                    return chatgpt_row_from_status(&status, &src);
                }
                return codex;
            }
        }
    }
    codex_chatgpt_row(r, src).await
}

async fn codex_chatgpt_row(r: &'static CliRow, src: String) -> Value {
    let Some(tool) = crate::cli_catalog::find(r.tool) else {
        return row_json(r, "off", None, None, false, Some("Not available on this computer."), vec![], &src);
    };
    let models: Vec<String> = tool.models.iter().map(|m| m.id.clone()).collect();
    let bin = tool.bin.clone();
    let tool_id = tool.id.clone();
    let signed_in_bin = bin.clone();
    let probe = tokio::task::spawn_blocking(move || {
        let installed = crate::cli_tools::resolve_binary(&signed_in_bin).is_some();
        if !installed {
            return (false, false, None);
        }
        let signed_in = crate::provider_routes::subscription_auth_check(&tool_id, &signed_in_bin);
        let who = if signed_in && tool_id == "claude-cli" {
            crate::provider_routes::claude_cli_status(&signed_in_bin, None).1
        } else if signed_in && tool_id == "codex-cli" {
            std::fs::read_to_string(crate::provider_routes::home_file(&[".codex", "auth.json"]))
                .ok()
                .and_then(|t| codex_plan_from_auth(&t))
        } else {
            None
        };
        (true, signed_in, who)
    })
    .await
    .unwrap_or((false, false, None));
    let (installed, signed_in, who) = probe;

    if !installed {
        return row_json(r, "off", None, None, false, Some("Not installed on this computer."), models, &src);
    }
    if signed_in {
        let found = match (r.id, who) {
            (_, Some(w)) => w,
            ("chatgpt", None) => "Signed in with Codex".to_string(),
            _ => "Signed in on this computer".to_string(),
        };
        return row_json(r, "on", Some(found), None, true, None, models, &src);
    }
    // A sign-in is running here, or just ended badly.
    match crate::cli_tools::login_view(&tool.id).await {
        Some((_, false, _)) => row_json(r, "connecting", None, None, true, None, models, &src),
        Some((view, true, false)) => {
            let stopped = view.get("error").and_then(Value::as_str) == Some("sign-in was stopped");
            let reason = if stopped {
                ("login_expired", "Sign-in expired")
            } else {
                ("could_not_start", "Could not start. Try again")
            };
            row_json(r, "needs", None, Some(reason), true, None, models, &src)
        }
        _ => row_json(r, "off", None, None, true, None, models, &src),
    }
}

async fn local_row(src: String) -> Value {
    let info = crate::cli_provider_detector::detect_ollama().await;
    let models: Vec<String> = info.models.unwrap_or_default().into_iter().map(|m| m.id).collect();
    let (state, found, available, why): (&str, Option<String>, bool, Option<&str>) = if info.available {
        ("on", Some(format!("{} on this computer", models.len().max(1)) + if models.len() == 1 { " model" } else { " models" }), true, None)
    } else if info.installed {
        ("off", None, true, None)
    } else {
        ("off", None, false, Some("Not installed on this computer."))
    };
    json!({
        "id": "local", "group": "brains", "state": state, "found": found,
        "reason": null, "reason_code": null, "runs_on": "computer",
        "available": available, "unavailable_reason": why, "fallbacks": [],
        "terms": "cleared", "saved": "device", "models": models, "source": src,
    })
}

async fn device_key_rows(src: &str) -> Vec<Value> {
    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;
    DEVICE_KEYS
        .iter()
        .filter(|(gizzi_id, ..)| connected.contains(*gizzi_id))
        .map(|(_, id, name)| {
            json!({
                "id": id, "group": "brains", "state": "on",
                "found": format!("Key saved on this computer · {name}"),
                "reason": null, "reason_code": null, "runs_on": "device",
                "available": true, "unavailable_reason": null, "fallbacks": [],
                "terms": "cleared", "saved": "device", "models": [], "source": src,
            })
        })
        .collect()
}

async fn row_for(id: &str) -> Option<Value> {
    let src = source();
    if let Some(r) = find_row(id) {
        return Some(cli_row(r, src).await);
    }
    if id == "local" {
        return Some(local_row(src).await);
    }
    device_key_rows(&src).await.into_iter().find(|v| v["id"] == id)
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn list_connections(headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.");
    }
    let src = source();
    let cli = futures::future::join_all(CLI_ROWS.iter().map(|r| cli_row(r, src.clone())));
    let (mut rows, local, keys) = tokio::join!(cli, local_row(src.clone()), device_key_rows(&src));
    rows.push(local);
    rows.extend(keys);
    Json(json!({ "version": 1, "connections": rows })).into_response()
}

#[derive(Debug, Default, Deserialize)]
struct ConnectBody {
    #[allow(dead_code)]
    way: Option<String>,
    accept_disclosure: Option<String>,
}

async fn connect(Path(id): Path<String>, headers: HeaderMap, body: Option<Json<ConnectBody>>) -> Response {
    if get_user(&headers).is_none() {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.");
    }
    if id == "chatgpt" {
        if let Some(bin) = crate::cloud_worker::gizzi_bin() {
            let accept = body.as_ref().and_then(|b| b.accept_disclosure.as_deref());
            return connect_chatgpt_with(&bin, accept).await;
        }
    }
    let (Some(r), true) = (find_row(&id), true) else {
        return err(StatusCode::NOT_FOUND, "unknown_connection", "Nothing here to connect.");
    };
    let Some(tool) = crate::cli_catalog::find(r.tool) else {
        return err(StatusCode::NOT_FOUND, "unknown_connection", "Nothing here to connect.");
    };
    let bin = tool.bin.clone();
    let tool_id = tool.id.clone();
    let already = tokio::task::spawn_blocking({
        let (tool_id, bin) = (tool_id.clone(), bin.clone());
        move || crate::provider_routes::subscription_auth_check(&tool_id, &bin)
    })
    .await
    .unwrap_or(false);
    if already {
        return Json(json!({ "state": "on" })).into_response();
    }
    let Some(binary_path) = crate::cli_tools::resolve_binary(&bin) else {
        return err(StatusCode::CONFLICT, "could_not_start", "Not installed on this computer.");
    };
    if tool.login.is_empty() {
        return err(StatusCode::CONFLICT, "could_not_start", "This one signs in outside Allternit.");
    }
    // Device-code sign-ins work from any device, so the remote argv is always right here.
    let cmd = crate::provider_routes::login_argv(&tool.id, &tool.login, true);
    match crate::cli_tools::start_login_with_env(&tool_id, &binary_path, &cmd.argv, cmd.hint, &cmd.env).await {
        Ok(view) => {
            let s = |k: &str| view.get(k).and_then(Value::as_str).map(str::to_string);
            let needs = view.get("needsCode").and_then(Value::as_bool).unwrap_or(false);
            let login = login_object(&tool_id, s("url").as_deref(), s("code").as_deref(), needs, chrono::Utc::now());
            Json(json!({ "login": login })).into_response()
        }
        Err(e) => {
            tracing::warn!("connection sign-in did not start for {id}: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could_not_start", "Could not start. Try again")
        }
    }
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    value: String,
}

async fn submit(Path(id): Path<String>, headers: HeaderMap, Json(body): Json<SubmitBody>) -> Response {
    if get_user(&headers).is_none() {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.");
    }
    let Some(r) = find_row(&id) else {
        return err(StatusCode::NOT_FOUND, "unknown_connection", "Nothing here to connect.");
    };
    if body.value.trim().is_empty() || body.value.len() > 4096 {
        return err(StatusCode::BAD_REQUEST, "invalid_value", "Paste the code from the sign-in page.");
    }
    match crate::cli_tools::submit_code(r.tool, &body.value).await {
        Ok(()) => {
            // Give the CLI a moment to write its sign-in before the row is read.
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            match row_for(&id).await {
                Some(row) => Json(row).into_response(),
                None => err(StatusCode::NOT_FOUND, "unknown_connection", "Nothing here to connect."),
            }
        }
        Err(_) => err(StatusCode::CONFLICT, "login_expired", "Sign-in expired"),
    }
}

async fn login_status(Path(id): Path<String>, headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.");
    }
    if id == "chatgpt" {
        if let Some(bin) = crate::cloud_worker::gizzi_bin() {
            let running = CHATGPT_SIGN_IN.lock().await.is_some();
            if let Some(status) = chatgpt_device_status(&bin).await {
                let word = chatgpt_login_word(&status);
                // A device sign-in this server started, or one already done: gizzi answers.
                if running || word == "done" {
                    if word != "pending" {
                        CHATGPT_SIGN_IN.lock().await.take();
                    }
                    let row = (word == "done").then(|| chatgpt_row_from_status(&status, &source()));
                    return Json(json!({ "status": word, "row": row })).into_response();
                }
            }
        }
    }
    let Some(r) = find_row(&id) else {
        return err(StatusCode::NOT_FOUND, "unknown_connection", "Nothing here to connect.");
    };
    let Some(tool) = crate::cli_catalog::find(r.tool) else {
        return err(StatusCode::NOT_FOUND, "unknown_connection", "Nothing here to connect.");
    };
    let (tool_id, bin) = (tool.id.clone(), tool.bin.clone());
    let signed_in = tokio::task::spawn_blocking({
        let (tool_id, bin) = (tool_id.clone(), bin);
        move || crate::provider_routes::subscription_auth_check(&tool_id, &bin)
    })
    .await
    .unwrap_or(false);
    if signed_in {
        crate::cli_tools::note_signed_in(&tool_id).await;
    }
    let view = crate::cli_tools::login_view(&tool_id).await;
    let err_text = view
        .as_ref()
        .and_then(|(v, ..)| v.get("error").and_then(Value::as_str).map(str::to_string));
    let session = view.as_ref().map(|(_, exited, success)| (*exited, *success, err_text.as_deref()));
    let status = login_status_word(signed_in, session);
    let row = if status == "done" { row_for(&id).await } else { None };
    Json(json!({ "status": status, "row": row })).into_response()
}

async fn disconnect(Path(id): Path<String>, headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.");
    }
    if id == "chatgpt" {
        if let Some(bin) = crate::cloud_worker::gizzi_bin() {
            let on = chatgpt_device_status(&bin).await.and_then(|s| s.get("state").and_then(Value::as_str).map(|s| s == "on")).unwrap_or(false);
            if on {
                // Revokes at OpenAI, then deletes the tokens on this computer.
                return match gizzi_json(&bin, &["connect", "disconnect", "chatgpt", "--json"], 45).await {
                    Some(v) if v["state"] == "off" => StatusCode::NO_CONTENT.into_response(),
                    _ => err(StatusCode::BAD_GATEWAY, "could_not_sign_out", "Could not sign out. Try again"),
                };
            }
        }
    }
    let Some(r) = find_row(&id) else {
        return err(StatusCode::NOT_FOUND, "unknown_connection", "Nothing here to disconnect.");
    };
    let (Some(tool), Some(argv)) = (crate::cli_catalog::find(r.tool), logout_argv(r.tool)) else {
        return err(StatusCode::BAD_REQUEST, "logout_unsupported", "Sign out inside that app.");
    };
    let Some(binary) = crate::cli_tools::resolve_binary(&tool.bin) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let run = tokio::process::Command::new(binary)
        .args(argv)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(std::time::Duration::from_secs(20), run).await {
        Ok(Ok(s)) if s.success() => StatusCode::NO_CONTENT.into_response(),
        _ => err(StatusCode::BAD_GATEWAY, "could_not_sign_out", "Could not sign out. Try again"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-11-13T10:00:00Z").unwrap().into()
    }

    #[test]
    fn codex_login_is_a_device_code() {
        let l = login_object("codex-cli", Some("https://auth.openai.com/codex/device"), Some("ZWBG-JYPGA"), false, t0());
        assert_eq!(l["kind"], "device_code");
        assert_eq!(l["code"], "ZWBG-JYPGA");
        assert_eq!(l["url"], "https://auth.openai.com/codex/device");
        assert!(l["url_with_code"].is_null());
        assert_eq!(l["expires_at"], "2026-11-13T10:15:00Z");
        assert_eq!(l["poll"], true);
    }

    #[test]
    fn grok_login_carries_the_url_with_the_code() {
        let url = "https://accounts.x.ai/oauth2/device?user_code=4782-CGRP";
        let l = login_object("grok", Some(url), Some("4782-CGRP"), false, t0());
        assert_eq!(l["kind"], "device_code");
        assert_eq!(l["url_with_code"], url);
    }

    #[test]
    fn claude_login_is_link_and_paste_even_before_the_prompt_shows() {
        let l = login_object("claude-cli", Some("https://claude.com/cai/oauth/authorize?code=true"), None, true, t0());
        assert_eq!(l["kind"], "link_paste");
        assert!(l["code"].is_null());
        let early = login_object("claude-cli", Some("https://claude.com/cai/oauth/authorize"), None, false, t0());
        assert_eq!(early["kind"], "link_paste");
    }

    #[test]
    fn status_words_follow_the_cli() {
        // Signed in wins over everything.
        assert_eq!(login_status_word(true, Some((false, false, None))), "done");
        assert_eq!(login_status_word(true, None), "done");
        // Still running.
        assert_eq!(login_status_word(false, Some((false, false, None))), "pending");
        // Ended: stopped at the 15 minute limit or replaced.
        assert_eq!(login_status_word(false, Some((true, false, Some("sign-in was stopped")))), "expired");
        // Ended with an error from the CLI.
        assert_eq!(login_status_word(false, Some((true, false, Some("denied")))), "failed");
        // No login here at all.
        assert_eq!(login_status_word(false, None), "expired");
    }

    #[test]
    fn plan_comes_from_the_codex_id_token() {
        let claims = r#"{"https://api.openai.com/auth":{"chatgpt_plan_type":"plus"}}"#;
        let b = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let jwt = format!("{}.{}.sig", b.encode(r#"{"alg":"none"}"#), b.encode(claims));
        let auth = format!(r#"{{"tokens":{{"id_token":"{jwt}"}}}}"#);
        assert_eq!(codex_plan_from_auth(&auth).as_deref(), Some("ChatGPT Plus"));
        assert_eq!(codex_plan_from_auth(r#"{"tokens":{}}"#), None);
        assert_eq!(codex_plan_from_auth("not json"), None);
    }

    #[tokio::test]
    async fn list_answers_with_runtime_rows_for_a_signed_in_user_and_401_without() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let app = connections_router::<()>();
        let none = app
            .clone()
            .oneshot(Request::get("/connections").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(none.status(), StatusCode::UNAUTHORIZED);
        let ok = app
            .oneshot(Request::get("/connections").header("x-allternit-user-id", "u1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(ok.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["version"], 1);
        let rows = v["connections"].as_array().unwrap();
        for id in ["claude-code", "codex", "gemini-cli", "cursor", "grok-cli", "kimi-cli", "chatgpt", "local"] {
            let row = rows.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("row {id} missing"));
            assert!(row["source"].as_str().unwrap().starts_with("runtime:"));
            assert!(["off", "connecting", "on", "needs"].contains(&row["state"].as_str().unwrap()));
        }
    }

    #[test]
    fn chatgpt_row_follows_the_device_sign_in() {
        let on = json!({"id":"chatgpt","state":"on","found":"ChatGPT Plus · a@b.c","models":["chatgpt/gpt-6.1-sol"],
            "disclosure":{"version":"2026-11-14.1","accepted_at":"2026-11-14T17:00:00Z"}});
        let row = chatgpt_row_from_status(&on, "runtime:r1");
        assert_eq!(row["state"], "on");
        assert_eq!(row["found"], "ChatGPT Plus · a@b.c");
        assert_eq!(row["runs_on"], "computer");
        assert_eq!(row["person_only"], true);
        assert_eq!(row["models"][0], "chatgpt/gpt-6.1-sol");
        assert_eq!(row["disclosure"]["accepted_at"], "2026-11-14T17:00:00Z");
        let expired = chatgpt_row_from_status(&json!({"state":"needs","reason_code":"login_expired"}), "runtime:r1");
        assert_eq!(expired["reason_code"], "login_expired");
        assert_eq!(expired["reason"], "Sign-in expired");
        assert_eq!(chatgpt_row_from_status(&json!({"state":"weird"}), "x")["state"], "off");
        assert_eq!(chatgpt_login_word(&on), "done");
        assert_eq!(chatgpt_login_word(&json!({"state":"connecting"})), "pending");
        assert_eq!(chatgpt_login_word(&json!({"state":"needs","reason_code":"login_expired"})), "expired");
        assert_eq!(chatgpt_login_word(&json!({"state":"needs","reason_code":"could_not_start"})), "failed");
        assert_eq!(chatgpt_login_word(&json!({"state":"off"})), "expired");
    }

    /// A stand-in for `gizzi`: the same JSON lines `gizzi connect … chatgpt --json` prints.
    #[cfg(unix)]
    fn fake_gizzi(dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = r#"#!/bin/sh
D="$(dirname "$0")"
DISC='{"version":"2026-11-14.1","title":"Before you connect your ChatGPT plan","body":["one","two","three","four"],"accept_label":"I understand, connect","decline_label":"Use a key instead"}'
if [ "$2" = "status" ]; then
  if [ -f "$D/accepted" ]; then AT='"2026-11-14T17:00:00Z"'; else AT=null; fi
  if [ -f "$D/running" ]; then ST=connecting; else ST=off; fi
  printf '{"id":"chatgpt","state":"%s","models":[],"disclosure":{"version":"2026-11-14.1","accepted_at":%s}}\n' "$ST" "$AT"
  exit 0
fi
for a in "$@"; do [ "$prev" = "--accept-disclosure" ] && [ "$a" = "2026-11-14.1" ] && touch "$D/accepted"; prev="$a"; done
if [ ! -f "$D/accepted" ]; then
  printf '{"state":"needs","reason_code":"disclosure_required","disclosure":%s}\n' "$DISC"
  exit 2
fi
touch "$D/running"
echo '{"state":"connecting","authorize_url":"https://auth.openai.com/api/accounts/authorize?client_id=dynamic_agent_client","redirect_uri":"http://127.0.0.1:1455/auth/callback","expires_at":"2026-11-14T18:00:00Z"}'
sleep 5
"#;
        let path = dir.join("gizzi");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn chatgpt_connect_shows_the_disclosure_before_any_sign_in() {
        let tmp = tempfile::tempdir().unwrap();
        let gizzi = fake_gizzi(tmp.path());
        let body = |r: Response| async move {
            let status = r.status();
            let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        };

        // No acceptance: 409 with the disclosure, the same shape as cloud-api (§9.2), and nothing started.
        let (status, v) = body(connect_chatgpt_with(&gizzi, None).await).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(v["error"]["code"], "disclosure_required");
        assert_eq!(v["error"]["disclosure"]["version"], "2026-11-14.1");
        assert_eq!(v["error"]["disclosure"]["title"], "Before you connect your ChatGPT plan");
        assert_eq!(v["error"]["disclosure"]["body"].as_array().unwrap().len(), 4);
        assert_eq!(v["error"]["disclosure"]["accept_label"], "I understand, connect");
        assert!(!tmp.path().join("running").exists());

        // A stale version is the same as none.
        let (status, _) = body(connect_chatgpt_with(&gizzi, Some("2026-01-01.1")).await).await;
        assert_eq!(status, StatusCode::CONFLICT);

        // The current version: gizzi records it and the sign-in starts as a browser redirect.
        let (status, v) = body(connect_chatgpt_with(&gizzi, Some(CHATGPT_DISCLOSURE_VERSION)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["login"]["kind"], "browser_redirect");
        assert!(v["login"]["url"].as_str().unwrap().starts_with("https://auth.openai.com/api/accounts/authorize"));
        assert_eq!(v["login"]["poll"], true);
        assert!(tmp.path().join("accepted").exists());
        CHATGPT_SIGN_IN.lock().await.take();
    }

    #[test]
    fn row_ids_resolve_to_catalog_tools_with_a_login() {
        for r in CLI_ROWS {
            assert!(crate::cli_catalog::find(r.tool).is_some(), "{} missing from the catalog", r.tool);
        }
        assert!(logout_argv("codex-cli").is_some());
        assert!(logout_argv("gemini-cli").is_none());
    }
}
