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
}

async fn connect(Path(id): Path<String>, headers: HeaderMap, _body: Option<Json<ConnectBody>>) -> Response {
    if get_user(&headers).is_none() {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.");
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
    fn row_ids_resolve_to_catalog_tools_with_a_login() {
        for r in CLI_ROWS {
            assert!(crate::cli_catalog::find(r.tool).is_some(), "{} missing from the catalog", r.tool);
        }
        assert!(logout_argv("codex-cli").is_some());
        assert!(logout_argv("gemini-cli").is_none());
    }
}
