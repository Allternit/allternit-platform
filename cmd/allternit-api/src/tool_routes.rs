//! Tool Execution Routes
//!
//! Provides a unified endpoint for executing agent tools.
//! Tools can be local (filesystem, shell, browser) or proxied to MCP servers.

use axum::{
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::AuthUser;
use crate::bot_desktop_input::{KeyboardInput, MouseInput, ShellInput};
use crate::computer_control::{execute_computer_tool, ComputerControlAction};
use crate::permission_policy::{evaluate, PermissionAction};
use crate::AppState;

// ─── Cache-control helpers ──────────────────────────────────────────────────

/// Character threshold above which a tool result is considered "large" and
/// may be annotated with `cache_control: { type: "ephemeral" }` when the
/// caller opts into caching. Aligns with Anthropic's prompt-caching minimum
/// of roughly 1k tokens (~4k characters of JSON).
const TOOL_RESULT_CACHE_THRESHOLD_CHARS: usize = 4_000;

/// Return an Anthropic-style ephemeral cache_control hint when the result is
/// large enough to benefit from prompt caching and the caller enabled it.
fn tool_result_cache_control(result: &Value, cache_enabled: bool) -> Option<Value> {
    if !cache_enabled {
        return None;
    }
    let size = serde_json::to_string(result).map(|s| s.len()).unwrap_or(0);
    if size >= TOOL_RESULT_CACHE_THRESHOLD_CHARS {
        Some(json!({"type": "ephemeral"}))
    } else {
        None
    }
}

// ─── Host-control hardening helpers ─────────────────────────────────────────

/// Tools that can execute code, mutate the host filesystem, or read sensitive
/// host state. These require `config.host_control_enabled()` to be true.
const HOST_CONTROL_TOOLS: &[&str] = &[
    "shell.exec",
    "shell.eval",
    "bash",
    "code_execution",
    "file.write",
    "file.remove",
    "system.env",
];

/// Returns true when a key looks like it carries a secret or credential.
pub(crate) fn is_secret_key(key: &str) -> bool {
    let upper = key.to_uppercase();
    [
        "SECRET",
        "TOKEN",
        "API_KEY",
        "PASSWORD",
        "PRIVATE",
        "CREDENTIAL",
        "ENCRYPTION_KEY",
        "AUTH",
        "PASSPHRASE",
    ]
    .iter()
    .any(|needle| upper.contains(needle))
}

/// Minimal, safe environment for subprocesses spawned by host-control tools.
/// Clears the parent environment so provider/API secrets are never inherited.
pub(crate) fn safe_subprocess_env() -> std::collections::HashMap<String, String> {
    crate::env_allowlist::minimal_child_env(None)
}

/// Snapshot of the current process environment with known secret keys removed.
fn scrubbed_env_snapshot() -> std::collections::HashMap<String, Option<String>> {
    std::env::vars()
        .filter(|(k, _)| !is_secret_key(k))
        .map(|(k, v)| (k, Some(v)))
        .collect()
}

// ─── Request/Response Types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct ExecuteToolRequest {
    /// Tool identifier (e.g. "shell.exec", "file.read", "browser.navigate", "mcp:<server>:<tool>")
    /// `tool_name` alias: the web/desktop surface (`native-agent-api.ts`,
    /// `recording.store.ts`) posts that key.
    #[serde(alias = "tool_name")]
    pub tool: String,
    /// Tool arguments (`parameters` alias, same reason as above).
    #[serde(alias = "parameters")]
    pub args: Value,
    /// Optional timeout in seconds (default: 30)
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Optional workspace context
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// When true, large tool results may include a `cache_control` hint for
    /// providers that support prompt caching (e.g. Anthropic).
    #[serde(default)]
    pub cache: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ExecuteToolResponse {
    pub success: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub execution_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn tool_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tools", get(list_tools))
        .route("/tools/execute", post(execute_tool))
        .route("/beta/approvals/:id/approve", post(approve_tool_execution))
        .route("/beta/approvals/:id/deny", post(deny_tool_execution))
}

// ─── Permission helpers ─────────────────────────────────────────────────────

/// Extract the file path a tool request targets, if any.
fn request_file_path(args: &Value) -> Option<&str> {
    args.get("path")
        .and_then(|v| v.as_str())
        .or_else(|| args.get("file_path").and_then(|v| v.as_str()))
}

/// Extract the network host a tool request targets, if any.
fn request_network_host(args: &Value) -> Option<String> {
    let url = args.get("url").and_then(|v| v.as_str())?;
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn execute_tool(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ExecuteToolRequest>,
) -> impl IntoResponse {
    let user = match headers
        .get("x-allternit-user-id")
        .and_then(|v| v.to_str().ok())
    {
        Some(user_id) => AuthUser {
            user_id: user_id.to_string(),
            email: headers
                .get("x-allternit-user-email")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            name: headers
                .get("x-allternit-user-name")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            avatar_url: None,
            tenant_id: headers
                .get("x-allternit-tenant-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            organization_id: headers
                .get("x-allternit-organization-id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            organization_role: headers
                .get("x-allternit-organization-role")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
            organization_slug: headers
                .get("x-allternit-organization-slug")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string()),
        },
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            );
        }
    };

    // Evaluate agent-level permission policy before executing the tool.
    let active_policy = state.config.active_permission_policy();
    let decision = evaluate(
        active_policy.as_ref(),
        &body.tool,
        request_file_path(&body.args),
        request_network_host(&body.args).as_deref(),
    );
    match decision {
        PermissionAction::Deny => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "Tool execution denied by permission policy",
                    "tool": body.tool,
                })),
            );
        }
        PermissionAction::Ask => {
            let approval_id = state.approval_store.create(&user.user_id, &body.tool, &body.args);
            info!("Policy asked for approval '{}' for tool '{}'", approval_id, body.tool);
            return (
                StatusCode::ACCEPTED,
                Json(json!({
                    "status": "approval_required",
                    "approval_id": approval_id,
                    "tool": body.tool,
                })),
            );
        }
        PermissionAction::Allow => {}
    }

    let start = std::time::Instant::now();
    info!("Executing tool '{}' for user '{}'", body.tool, user.user_id);

    let cache_enabled = body.cache.unwrap_or(false);

    let org_id = user.organization_id.as_deref().or(user.tenant_id.as_deref());
    let result = match execute_tool_internal(&state, &body, &user.user_id, org_id).await {
        Ok(result) => {
            let cache_control = tool_result_cache_control(&result, cache_enabled);
            json!({
                "success": true,
                "result": result,
                "execution_time_ms": start.elapsed().as_millis() as u64,
                "cache_control": cache_control,
            })
        }
        Err(e) => {
            warn!("Tool '{}' failed: {}", body.tool, e);
            json!({
                "success": false,
                "error": e,
                "execution_time_ms": start.elapsed().as_millis() as u64,
            })
        }
    };

    (StatusCode::OK, Json(result))
}

async fn approve_tool_execution(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.approval_store.approve(&id) {
        (
            StatusCode::OK,
            Json(json!({ "approval_id": id, "status": "approved" })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Approval request not found" })),
        )
    }
}

async fn deny_tool_execution(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.approval_store.deny(&id) {
        (
            StatusCode::OK,
            Json(json!({ "approval_id": id, "status": "denied" })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Approval request not found" })),
        )
    }
}

// ─── Tool Implementations ───────────────────────────────────────────────────

/// `pub(crate)`: also called by `mcp_server_routes.rs`'s `tools/call` handler,
/// so the same tool set is reachable both as a plain REST call and as an
/// MCP server tool — one registry, two front doors, same auth gate.
pub(crate) async fn execute_tool_internal(
    state: &Arc<AppState>,
    request: &ExecuteToolRequest,
    user_id: &str,
    tenant_id: Option<&str>,
) -> Result<Value, String> {
    // Host-control tools are opt-in. Refuse them with a clear message so callers
    // know the feature flag must be enabled.
    if HOST_CONTROL_TOOLS.contains(&request.tool.as_str()) && !state.config.host_control_enabled() {
        return Err(format!(
            "Host-control tool '{}' is disabled. Set ALLTERNIT_HOST_CONTROL_ENABLED=true or hostControlEnabled in config to enable.",
            request.tool
        ));
    }

    // Server-side tools take precedence over native tools when registered for
    // the caller's organization. They run inside the platform sandbox.
    if let Some(org_id) = tenant_id {
        match crate::server_tool_routes::execute_server_tool(state, &request.tool, &request.args, org_id).await {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(e) => return Err(e),
        }
    }

    match request.tool.as_str() {
        // ── Shell Execution ──────────────────────────────────────────────────
        "shell.exec" => shell_exec(request).await,
        "shell.eval" => shell_exec(request).await,
        "bash" => bash_exec(request).await,

        // ── Code Execution ───────────────────────────────────────────────────
        "code_execution" => code_execution(request).await,

        // ── File System ──────────────────────────────────────────────────────
        "file.read" => file_read(request).await,
        "file.write" => file_write(request).await,
        "file.list" => file_list(request).await,
        "file.exists" => file_exists(request).await,
        "file.remove" => file_remove(request).await,

        // ── System Info ──────────────────────────────────────────────────────
        "system.info" => system_info(request).await,
        "system.env" => system_env(request).await,

        // ── Network ──────────────────────────────────────────────────────────
        "http.get" => http_get(request).await,
        "http.post" => http_post(request).await,

        // ── Markdown Conversion (office engine) ──────────────────────────────
        "document_to_markdown" => document_to_markdown(state, request, user_id).await,
        "url_to_markdown" => url_to_markdown(state, request).await,

        // ── Time ─────────────────────────────────────────────────────────────
        "time.now" => time_now(request).await,

        // ── Echo / Debug ─────────────────────────────────────────────────────
        "echo" => Ok(request.args.clone()),

        // ── HAR-derived API capture ──────────────────────────────────────────
        "api_capture_record" => api_capture_record(state, request, user_id).await,
        "api_capture_stop" => api_capture_stop(state, request, user_id).await,
        "api_capture_replay" => api_capture_replay(state, request, user_id).await,

        // ── Computer / Desktop Cloud control ─────────────────────────────────
        "computer_screenshot" => computer_screenshot_tool(state, user_id, &request.args).await,
        "computer_mouse" => computer_mouse_tool(state, user_id, &request.args).await,
        "computer_keyboard" => computer_keyboard_tool(state, user_id, &request.args).await,
        "computer_shell" => computer_shell_tool(state, user_id, &request.args).await,
        "computer_file_read" => computer_file_read_tool(state, user_id, &request.args).await,
        "computer_file_write" => computer_file_write_tool(state, user_id, &request.args).await,

        // ── Computer lifecycle (route-handler parity, Phase 5) ───────────────
        "computer_create" => computer_create_tool(state, user_id, tenant_id, &request.args).await,
        "computer_list" => computer_list_tool(state, user_id, tenant_id, &request.args).await,
        "computer_start" => computer_start_tool(state, user_id, tenant_id, &request.args).await,
        "computer_stop" => computer_stop_tool(state, user_id, tenant_id, &request.args).await,
        "computer_resize" => computer_resize_tool(state, user_id, tenant_id, &request.args).await,
        "computer_clone" => computer_clone_tool(state, user_id, tenant_id, &request.args).await,

        // ── Unknown ──────────────────────────────────────────────────────────
        _ => {
            if request.tool.starts_with("mcp:") {
                Err(format!(
                    "MCP tool execution not yet implemented for '{}'",
                    request.tool
                ))
            } else {
                Err(format!("Unknown tool: '{}'", request.tool))
            }
        }
    }
}

// ── Computer control tools ──────────────────────────────────────────────────

fn require_computer_id(args: &Value) -> Result<String, String> {
    args.get("computer_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Missing 'computer_id' argument".to_string())
}

/// Optional action-hash grant presented by the caller for risky/irreversible
/// computer actions (see `aci_approvals`). Enforcement is server-side: a
/// grant minted elsewhere, already consumed, expired, or bound to a different
/// action hash is denied by the gateway, not the client.
fn request_approval_id(args: &Value) -> Option<String> {
    args.get("approval_id")
        .or_else(|| args.get("approvalId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// screenshot / mouse / keyboard are aliases over the computer toolset
// (`computer_toolset::execute`, contract `allternit.computer.v1`): one
// executor for every caller, with its lease, approval, batch and receipt
// rules. Coordinates are in the screenshot's frame (`screen.frame_width` x
// `screen.frame_height` in the reply), same as the toolset. shell and file
// have no contract member yet, so they stay on `computer_control`.

/// Map the old `computer_mouse` input onto one toolset member + input.
fn mouse_to_toolset(input: &MouseInput) -> Result<(&'static str, Value), String> {
    let point = match (input.x, input.y) {
        (Some(x), Some(y)) => Some(json!([x, y])),
        _ => None,
    };
    let with_point = |member: &'static str| -> Result<(&'static str, Value), String> {
        let coordinate = point.clone().ok_or_else(|| format!("{} requires x and y", member))?;
        Ok((member, json!({ "coordinate": coordinate })))
    };
    match input.action.to_lowercase().as_str() {
        "move" => with_point("mouse_move"),
        "click" => match input.button.as_deref().unwrap_or("left") {
            "right" => with_point("right_click"),
            "middle" => with_point("middle_click"),
            _ => with_point("left_click"),
        },
        "rightclick" => with_point("right_click"),
        "doubleclick" => with_point("double_click"),
        "mousedown" => Ok(("left_mouse_down", json!({}))),
        "mouseup" => Ok(("left_mouse_up", json!({}))),
        "drag" => match (input.x, input.y, input.end_x, input.end_y) {
            (Some(x), Some(y), Some(ex), Some(ey)) => Ok((
                "left_click_drag",
                json!({ "start_coordinate": [x, y], "coordinate": [ex, ey] }),
            )),
            _ => Err("drag requires x, y, end_x, end_y".into()),
        },
        "scroll" => {
            let direction = match input.button.as_deref().unwrap_or("down") {
                "up" => "up",
                "down" => "down",
                other => return Err(format!("scroll button must be up or down, got {}", other)),
            };
            let amount = input.amount.unwrap_or(3);
            if amount <= 0 {
                return Err("scroll amount must be positive".into());
            }
            let mut out = json!({ "scroll_direction": direction, "scroll_amount": amount });
            if let Some(coordinate) = point.clone() {
                out["coordinate"] = coordinate;
            }
            Ok(("scroll", out))
        }
        other => Err(format!(
            "unknown mouse action '{}' (valid: move, click, rightclick, doubleclick, mousedown, mouseup, drag, scroll)",
            other
        )),
    }
}

/// Map the old `computer_keyboard` input onto one toolset member + input.
fn keyboard_to_toolset(input: &KeyboardInput) -> Result<(&'static str, Value), String> {
    match input.action.to_lowercase().as_str() {
        "type" => Ok(("type", json!({ "text": input.text.clone().unwrap_or_default() }))),
        "key" => {
            let key = input.key.clone().or_else(|| input.text.clone()).ok_or("key requires 'key'")?;
            Ok(("key", json!({ "text": key })))
        }
        other => Err(format!("unknown keyboard action '{}' (valid: type, key)", other)),
    }
}

/// Run one `allternit.computer.v1` member through the toolset executor.
async fn run_computer_member(
    state: &Arc<AppState>,
    tool: &str,
    user_id: &str,
    args: &Value,
    member: &str,
    input: Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let request: crate::computer_toolset::ToolsetRequest = serde_json::from_value(json!({
        "toolset": "computer",
        "member": member,
        "input": input,
        "approval_grant": request_approval_id(args),
    }))
    .map_err(|e| format!("{} request: {}", tool, e))?;
    let user = tool_auth_user(user_id, None);
    let (status, body) =
        crate::computer_toolset::execute(state, &user, &computer_id, &HeaderMap::new(), request).await;
    let is_error = body.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    if status.is_success() && !is_error {
        Ok(body)
    } else {
        Err(format!("{} failed ({}): {}", tool, status, body))
    }
}

async fn computer_screenshot_tool(state: &Arc<AppState>, user_id: &str, args: &Value) -> Result<Value, String> {
    run_computer_member(state, "computer_screenshot", user_id, args, "screenshot", json!({})).await
}

async fn computer_mouse_tool(state: &Arc<AppState>, user_id: &str, args: &Value) -> Result<Value, String> {
    let input: MouseInput = serde_json::from_value(args.clone()).map_err(|e| format!("invalid mouse input: {}", e))?;
    let (member, toolset_input) = mouse_to_toolset(&input)?;
    run_computer_member(state, "computer_mouse", user_id, args, member, toolset_input).await
}

async fn computer_keyboard_tool(state: &Arc<AppState>, user_id: &str, args: &Value) -> Result<Value, String> {
    let input: KeyboardInput =
        serde_json::from_value(args.clone()).map_err(|e| format!("invalid keyboard input: {}", e))?;
    let (member, toolset_input) = keyboard_to_toolset(&input)?;
    run_computer_member(state, "computer_keyboard", user_id, args, member, toolset_input).await
}

async fn computer_shell_tool(
    state: &AppState,
    user_id: &str,
    args: &Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let approval_id = request_approval_id(args);
    let input: ShellInput = serde_json::from_value(args.clone())
        .map_err(|e| format!("invalid shell input: {}", e))?;
    execute_computer_tool(
        &state,
        user_id,
        &computer_id,
        ComputerControlAction::Shell(input),
        approval_id.as_deref(),
    )
    .await
    .map_err(|(status, body)| format!("computer_shell failed ({}): {}", status, body))
}

async fn computer_file_read_tool(
    state: &AppState,
    user_id: &str,
    args: &Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'path' argument".to_string())?;
    execute_computer_tool(
        &state,
        user_id,
        &computer_id,
        ComputerControlAction::FileRead {
            path: path.to_string(),
        },
        None,
    )
    .await
    .map_err(|(status, body)| format!("computer_file_read failed ({}): {}", status, body))
}

async fn computer_file_write_tool(
    state: &AppState,
    user_id: &str,
    args: &Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let approval_id = request_approval_id(args);
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'path' argument".to_string())?;
    let content_base64 = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'content' argument (base64)".to_string())?;
    execute_computer_tool(
        &state,
        user_id,
        &computer_id,
        ComputerControlAction::FileWrite {
            path: path.to_string(),
            content_base64: content_base64.to_string(),
        },
        approval_id.as_deref(),
    )
    .await
    .map_err(|(status, body)| format!("computer_file_write failed ({}): {}", status, body))
}

// ── Computer lifecycle tools (Phase 5 catalog parity) ───────────────────────
//
// These tools call the same route handlers that serve `/api/v1/computers*` —
// one implementation, two front doors. Risky lifecycle actions are gated with
// the same ACI confirmation enforcement as the control tools (the REST
// lifecycle routes currently perform no ACI check; the tool surface fails
// closed instead, so the tool path is never weaker than any other entry
// point). `approval_id` is threaded exactly like the control tools thread it.

/// Build the AuthUser the route handlers expect from the tool-call identity.
/// `tenant_id` carries the caller's org id (see `execute_tool`).
fn tool_auth_user(user_id: &str, tenant_id: Option<&str>) -> AuthUser {
    AuthUser {
        user_id: user_id.to_string(),
        organization_id: tenant_id.map(|s| s.to_string()),
        tenant_id: None,
        email: None,
        name: None,
        avatar_url: None,
        organization_role: None,
        organization_slug: None,
    }
}

/// ACI gate for risky lifecycle actions, reusing `aci_safety::enforce_confirmation`
/// with the same taxonomy as the control routes. The denial payload (including
/// the fresh `approval_id` / `action_hash` for the handoff endpoints) is
/// surfaced verbatim in the tool error.
fn enforce_lifecycle_confirmation(
    state: &AppState,
    user_id: &str,
    route: &str,
    descriptor: &Value,
    approval_id: Option<&str>,
) -> Result<(), String> {
    crate::aci_safety::enforce_confirmation(
        &state.approval_store,
        user_id,
        route,
        crate::aci_safety::ConfirmationClass::Risky,
        descriptor,
        approval_id,
    )
    .map_err(|denial| format!("{} denied ({}): {}", route, denial.status, denial.body))
}

/// Map a route-handler Response into the tool Result shape: success → parsed
/// JSON body; failure → `status + body` error string.
async fn computer_route_result(tool: &str, response: Response) -> Result<Value, String> {
    let status = response.status();
    let (_parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024)
        .await
        .map_err(|e| format!("{} failed to read response body: {}", tool, e))?;
    let text = String::from_utf8_lossy(&bytes);
    let value: Value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
    };
    if status.is_success() {
        Ok(value)
    } else {
        Err(format!("{} failed ({}): {}", tool, status, value))
    }
}

/// Arguments minus the approval-grant keys, as a deterministic descriptor
/// payload (serde_json maps are key-sorted).
fn approval_free_args(args: &Value) -> Value {
    let mut descriptor = args.clone();
    if let Some(map) = descriptor.as_object_mut() {
        map.remove("approval_id");
        map.remove("approvalId");
    }
    descriptor
}

async fn computer_create_tool(
    state: &Arc<AppState>,
    user_id: &str,
    tenant_id: Option<&str>,
    args: &Value,
) -> Result<Value, String> {
    let req: crate::computer_routes::CreateComputerRequest =
        serde_json::from_value(args.clone()).map_err(|e| format!("invalid create arguments: {}", e))?;
    let descriptor = json!({
        "tool": "computer_create",
        "request": approval_free_args(args),
    });
    enforce_lifecycle_confirmation(state, user_id, "computers.create", &descriptor, request_approval_id(args).as_deref())?;
    let user = tool_auth_user(user_id, tenant_id);
    let response = crate::computer_routes::create_computer(
        State(state.clone()),
        Extension(user),
        Json(req),
    )
    .await
    .into_response();
    computer_route_result("computer_create", response).await
}

async fn computer_list_tool(
    state: &Arc<AppState>,
    user_id: &str,
    tenant_id: Option<&str>,
    args: &Value,
) -> Result<Value, String> {
    let query: crate::computer_routes::ListComputersQuery =
        serde_json::from_value(args.clone()).map_err(|e| format!("invalid list arguments: {}", e))?;
    let user = tool_auth_user(user_id, tenant_id);
    let response = crate::computer_routes::list_computers(
        State(state.clone()),
        Extension(user),
        Query(query),
    )
    .await
    .into_response();
    computer_route_result("computer_list", response).await
}

async fn computer_start_tool(
    state: &Arc<AppState>,
    user_id: &str,
    tenant_id: Option<&str>,
    args: &Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let descriptor = json!({ "tool": "computer_start", "computer_id": computer_id });
    enforce_lifecycle_confirmation(state, user_id, "computers.start", &descriptor, request_approval_id(args).as_deref())?;
    let user = tool_auth_user(user_id, tenant_id);
    let response = crate::computer_routes::start_computer(
        State(state.clone()),
        Extension(user),
        Path(computer_id),
    )
    .await
    .into_response();
    computer_route_result("computer_start", response).await
}

async fn computer_stop_tool(
    state: &Arc<AppState>,
    user_id: &str,
    tenant_id: Option<&str>,
    args: &Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let descriptor = json!({ "tool": "computer_stop", "computer_id": computer_id });
    enforce_lifecycle_confirmation(state, user_id, "computers.stop", &descriptor, request_approval_id(args).as_deref())?;
    let user = tool_auth_user(user_id, tenant_id);
    let response = crate::computer_routes::stop_computer(
        State(state.clone()),
        Extension(user),
        Path(computer_id),
    )
    .await
    .into_response();
    computer_route_result("computer_stop", response).await
}

async fn computer_resize_tool(
    state: &Arc<AppState>,
    user_id: &str,
    tenant_id: Option<&str>,
    args: &Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let req: crate::computer_routes::ResizeComputerRequest =
        serde_json::from_value(args.clone()).map_err(|e| format!("invalid resize arguments: {}", e))?;
    let descriptor = json!({
        "tool": "computer_resize",
        "computer_id": computer_id,
        "request": approval_free_args(args),
    });
    enforce_lifecycle_confirmation(state, user_id, "computers.resize", &descriptor, request_approval_id(args).as_deref())?;
    let user = tool_auth_user(user_id, tenant_id);
    let response = crate::computer_routes::resize_computer(
        State(state.clone()),
        Extension(user),
        Path(computer_id),
        Json(req),
    )
    .await;
    computer_route_result("computer_resize", response).await
}

async fn computer_clone_tool(
    state: &Arc<AppState>,
    user_id: &str,
    tenant_id: Option<&str>,
    args: &Value,
) -> Result<Value, String> {
    let computer_id = require_computer_id(args)?;
    let req: crate::computer_routes::CloneComputerRequest =
        serde_json::from_value(args.clone()).map_err(|e| format!("invalid clone arguments: {}", e))?;
    let descriptor = json!({
        "tool": "computer_clone",
        "computer_id": computer_id,
        "request": approval_free_args(args),
    });
    enforce_lifecycle_confirmation(state, user_id, "computers.clone", &descriptor, request_approval_id(args).as_deref())?;
    let user = tool_auth_user(user_id, tenant_id);
    let response = crate::computer_routes::clone_computer(
        State(state.clone()),
        Extension(user),
        Path(computer_id),
        Json(req),
    )
    .await;
    computer_route_result("computer_clone", response).await
}

// ── Shell ───────────────────────────────────────────────────────────────────

async fn shell_exec(request: &ExecuteToolRequest) -> Result<Value, String> {
    run_shell(
        request,
        request.timeout.unwrap_or(30),
        request.args.get("cwd").and_then(|v| v.as_str()),
    )
    .await
}

async fn bash_exec(request: &ExecuteToolRequest) -> Result<Value, String> {
    let timeout = request
        .args
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| request.timeout.unwrap_or(30));
    let restart = request
        .args
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // restart is currently advisory: each API invocation spawns a fresh process,
    // so a new shell is already provided. We include it in the schema for callers
    // trained on the WebVM contract.
    let _ = restart;
    run_shell(request, timeout, None).await
}

async fn run_shell(
    request: &ExecuteToolRequest,
    timeout_secs: u64,
    cwd: Option<&str>,
) -> Result<Value, String> {
    let command = request
        .args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'command' argument")?;

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .env_clear()
        .envs(safe_subprocess_env());

    if let Some(dir) = cwd {
        cmd.current_dir(std::path::PathBuf::from(dir));
    } else if let Some(dir) = std::env::current_dir().ok() {
        cmd.current_dir(dir);
    }

    let output = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output())
        .await
        .map_err(|_| "Shell command timed out")?
        .map_err(|e| format!("Failed to execute shell command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(json!({
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": output.status.code(),
        "success": output.status.success(),
    }))
}

async fn code_execution(request: &ExecuteToolRequest) -> Result<Value, String> {
    let language = request
        .args
        .get("language")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'language' argument")?;
    let code = request
        .args
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'code' argument")?;
    let timeout_secs = request
        .args
        .get("timeout_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| request.timeout.unwrap_or(30));

    let (program, flag) = match language.to_lowercase().as_str() {
        "python" | "python3" => ("python3", "-c"),
        "node" | "javascript" | "js" => ("node", "-e"),
        "bash" | "sh" => ("bash", "-c"),
        "rust" | "rs" => ("rustc", "--"),
        _ => return Err(format!("Unsupported language: {}", language)),
    };

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new(program)
            .arg(flag)
            .arg(code)
            .env_clear()
            .envs(safe_subprocess_env())
            .output(),
    )
    .await
    .map_err(|_| "Code execution timed out")?
    .map_err(|e| format!("Failed to execute code: {}", e))?;

    Ok(json!({
        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
        "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
        "exit_code": output.status.code(),
        "success": output.status.success(),
        "language": language,
    }))
}

// ── File System ─────────────────────────────────────────────────────────────

async fn file_read(request: &ExecuteToolRequest) -> Result<Value, String> {
    let path = request
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;

    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))?;

    Ok(json!({ "content": content, "path": path }))
}

async fn file_write(request: &ExecuteToolRequest) -> Result<Value, String> {
    let path = request
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let content = request
        .args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'content' argument")?;

    tokio::fs::write(path, content)
        .await
        .map_err(|e| format!("Failed to write file: {}", e))?;

    Ok(json!({ "path": path, "written": true }))
}

async fn file_list(request: &ExecuteToolRequest) -> Result<Value, String> {
    let path = request
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(path)
        .await
        .map_err(|e| format!("Failed to read directory: {}", e))?;

    while let Some(entry) = dir
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read directory entry: {}", e))?
    {
        let meta = entry.metadata().await.ok();
        entries.push(json!({
            "name": entry.file_name().to_string_lossy().to_string(),
            "path": entry.path().to_string_lossy().to_string(),
            "is_file": meta.as_ref().map(|m| m.is_file()).unwrap_or(false),
            "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
            "size": meta.as_ref().map(|m| m.len()),
        }));
    }

    Ok(json!({ "entries": entries, "path": path }))
}

async fn file_exists(request: &ExecuteToolRequest) -> Result<Value, String> {
    let path = request
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;

    let exists = tokio::fs::metadata(path).await.is_ok();
    Ok(json!({ "exists": exists, "path": path }))
}

async fn file_remove(request: &ExecuteToolRequest) -> Result<Value, String> {
    let path = request
        .args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;

    tokio::fs::remove_file(path)
        .await
        .map_err(|e| format!("Failed to remove file: {}", e))?;

    Ok(json!({ "removed": true, "path": path }))
}

// ── System Info ─────────────────────────────────────────────────────────────

async fn system_info(_request: &ExecuteToolRequest) -> Result<Value, String> {
    Ok(json!({
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "hostname": hostname::get().ok().map(|h| h.to_string_lossy().to_string()),
        "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
        "pid": std::process::id(),
    }))
}

async fn system_env(request: &ExecuteToolRequest) -> Result<Value, String> {
    if let Some(key) = request.args.get("key").and_then(|v| v.as_str()) {
        let value = std::env::var(key).ok();
        // Never return the real value of a secret-looking variable.
        let value = value.map(|v| {
            if is_secret_key(key) {
                "***".to_string()
            } else {
                v
            }
        });
        Ok(json!({ "key": key, "value": value }))
    } else {
        let envs = scrubbed_env_snapshot();
        Ok(json!({ "env": envs }))
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────────

async fn http_get(request: &ExecuteToolRequest) -> Result<Value, String> {
    let url = request
        .args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' argument")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            request.timeout.unwrap_or(30),
        ))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("HTTP GET failed: {}", e))?;

    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    Ok(json!({
        "status": status,
        "body": body,
    }))
}

async fn http_post(request: &ExecuteToolRequest) -> Result<Value, String> {
    let url = request
        .args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' argument")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            request.timeout.unwrap_or(30),
        ))
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.post(url);
    if let Some(body) = request.args.get("body") {
        req = req.json(body);
    }
    if let Some(headers) = request.args.get("headers") {
        if let Some(headers) = headers.as_object() {
            for (k, v) in headers {
                if let Some(v) = v.as_str() {
                    req = req.header(k, v);
                }
            }
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("HTTP POST failed: {}", e))?;

    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    Ok(json!({
        "status": status,
        "body": body,
    }))
}

// ── Markdown Conversion (office engine) ─────────────────────────────────────

/// Upper bound on markdown returned as a tool result. The office engine can
/// emit book-length GFM; tool results feed an LLM context, so long output is
/// truncated at a char boundary with an explicit note instead of failing.
const MAX_TOOL_MARKDOWN_CHARS: usize = 30_000;

/// Tool descriptors for the built-in markdown conversion tools, shared by the
/// REST tool list (`list_tools`), the MCP server catalog
/// (`mcp_server_routes.rs`), and the agents-v1 deferred-tool listing
/// (`agents_v1_routes.rs`) so every surface advertises the same
/// name/description/schema.
pub(crate) fn markdown_builtin_tools() -> Vec<Value> {
    vec![
        json!({
            "id": "document_to_markdown",
            "label": "Document to Markdown",
            "description": "Convert an uploaded document or artifact (PDF, DOCX, PPTX, XLSX, CSV, and more) into GitHub-flavored Markdown that can be read and quoted in chat.",
            "parameters": {
                "type": "object",
                "properties": {
                    "uploadId": {
                        "type": "string",
                        "description": "ID of a file uploaded through /api/v1/uploads (the composer attachment flow)."
                    },
                    "artifactId": {
                        "type": "string",
                        "description": "ID of an artifact whose text sections should be assembled into a single Markdown document."
                    }
                }
            }
        }),
        json!({
            "id": "url_to_markdown",
            "label": "URL to Markdown",
            "description": "Fetch a web page or a document hosted at a URL and convert it into GitHub-flavored Markdown.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The http(s) URL to fetch and convert."
                    }
                },
                "required": ["url"]
            }
        }),
    ]
}

/// Truncate over-long markdown at a char boundary, appending a note so the
/// caller knows the output is partial. Returns (text, was_truncated).
fn truncate_markdown(markdown: &str) -> (String, bool) {
    let total = markdown.chars().count();
    if total <= MAX_TOOL_MARKDOWN_CHARS {
        return (markdown.to_string(), false);
    }
    let head: String = markdown.chars().take(MAX_TOOL_MARKDOWN_CHARS).collect();
    (
        format!(
            "{head}\n\n_[Output truncated — showing the first {MAX_TOOL_MARKDOWN_CHARS} of {total} characters.]_"
        ),
        true,
    )
}

/// Convert the office engine's artifact-shaped JSON into a compact tool
/// result, truncating the markdown payload when needed.
fn markdown_tool_result(body: &Value) -> Result<Value, String> {
    let markdown = body
        .get("markdown")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Office engine response did not include markdown".to_string())?;
    let (markdown, truncated) = truncate_markdown(markdown);
    Ok(json!({
        "markdown": markdown,
        "truncated": truncated,
        "title": body.get("title").cloned().unwrap_or(Value::Null),
        "format": body.get("format").cloned().unwrap_or(Value::Null),
        "sourceUrl": body.get("sourceUrl").cloned().unwrap_or(Value::Null),
    }))
}

/// Read an office-engine response: success → truncated tool result, failure →
/// the engine's own error detail.
async fn engine_markdown_response(resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to read office engine response: {}", e))?;
    if !status.is_success() {
        let detail = body
            .get("detail")
            .or_else(|| body.get("error"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!(
            "Office engine conversion failed ({}): {}",
            status, detail
        ));
    }
    markdown_tool_result(&body)
}

fn office_engine_client(request: &ExecuteToolRequest) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            request.timeout.unwrap_or(60),
        ))
        .build()
        .map_err(|e| e.to_string())
}

/// Pull an artifact's text sections out of the local store and assemble them
/// as a single GFM document. Scoped to the calling user, same as the artifact
/// REST routes. Returns (document, title).
///
/// Unlike uploads, artifacts are already text, so this path does NOT round-trip
/// through the office engine: anydoc only handles binary document formats
/// (docx/pdf/xlsx/…) and rejects plain-text payloads as 415.
async fn read_artifact_as_document(
    state: &AppState,
    artifact_id: &str,
    user_id: &str,
) -> Result<(String, String), String> {
    let db = state.db.clone();
    let artifact_id = artifact_id.to_string();
    let user_id = user_id.to_string();

    tokio::task::spawn_blocking(move || {
        let conn = db
            .connect()
            .map_err(|e| format!("Failed to open database: {}", e))?;

        let title: Option<String> = conn
            .query_row(
                "SELECT title FROM artifacts WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![&artifact_id, &user_id],
                |row| row.get(0),
            )
            .ok();

        let title = title.ok_or_else(|| format!("Artifact not found: '{}'", artifact_id))?;

        let mut stmt = conn
            .prepare(
                "SELECT heading, body FROM artifact_sections
                 WHERE artifact_id = ?1 ORDER BY position ASC, created_at ASC",
            )
            .map_err(|e| format!("Failed to query artifact sections: {}", e))?;
        let sections: Vec<(String, String)> = stmt
            .query_map(rusqlite::params![&artifact_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("Failed to read artifact sections: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read artifact sections: {}", e))?;

        let mut document = format!("# {}\n", title);
        for (heading, body) in sections {
            document.push_str(&format!("\n## {}\n\n{}\n", heading, body));
        }

        Ok((document, title))
    })
    .await
    .map_err(|e| format!("Artifact read task failed: {}", e))?
}

async fn document_to_markdown(
    state: &AppState,
    request: &ExecuteToolRequest,
    user_id: &str,
) -> Result<Value, String> {
    let upload_id = request.args.get("uploadId").and_then(|v| v.as_str());
    let artifact_id = request.args.get("artifactId").and_then(|v| v.as_str());

    match (upload_id, artifact_id) {
        (Some(id), None) => {
            let (bytes, filename, _media_type) = crate::upload_routes::read_upload(id)
                .ok_or_else(|| format!("Upload not found: '{}'", id))?;

            let target = format!(
                "{}/markdown",
                state.config.office_engine_url().trim_end_matches('/')
            );
            let resp = office_engine_client(request)?
                .post(&target)
                .header("x-office-filename", &filename)
                .body(bytes)
                .send()
                .await
                .map_err(|e| format!("Cannot reach the office engine: {}", e))?;

            engine_markdown_response(resp).await
        }
        (None, Some(id)) => {
            let (document, title) = read_artifact_as_document(state, id, user_id).await?;
            let (markdown, truncated) = truncate_markdown(&document);
            Ok(json!({
                "markdown": markdown,
                "truncated": truncated,
                "title": title,
                "format": "artifact",
                "sourceUrl": Value::Null,
            }))
        }
        _ => Err("Provide exactly one of 'uploadId' or 'artifactId'".to_string()),
    }
}

async fn url_to_markdown(state: &AppState, request: &ExecuteToolRequest) -> Result<Value, String> {
    let url = request
        .args
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("Missing 'url' argument")?;

    let target = format!(
        "{}/markdown-url",
        state.config.office_engine_url().trim_end_matches('/')
    );
    let resp = office_engine_client(request)?
        .post(&target)
        .json(&json!({ "url": url }))
        .send()
        .await
        .map_err(|e| format!("Cannot reach the office engine: {}", e))?;

    engine_markdown_response(resp).await
}

// ── Time ────────────────────────────────────────────────────────────────────

async fn time_now(_request: &ExecuteToolRequest) -> Result<Value, String> {
    let now = chrono::Utc::now();
    Ok(json!({
        "iso": now.to_rfc3339(),
        "timestamp": now.timestamp(),
        "date": now.format("%Y-%m-%d").to_string(),
        "time": now.format("%H:%M:%S").to_string(),
    }))
}

// ─── HAR-derived API capture tools ───────────────────────────────────────────

async fn api_capture_record(
    state: &AppState,
    request: &ExecuteToolRequest,
    user_id: &str,
) -> Result<Value, String> {
    let domain = request.args.get("domain").and_then(|v| v.as_str());
    let source = request.args.get("source").and_then(|v| v.as_str());

    let session_id = crate::har_api_service::create_capture_session(
        &state.db,
        user_id,
        domain,
        source,
    )
    .await?;

    // If the caller already has a HAR (e.g. from an upload or extension),
    // stop the session immediately and persist the contract.
    if let Some(har) = request.args.get("har").and_then(|v| v.as_str()) {
        let result = crate::har_api_service::stop_capture_session(&state.db, &session_id, Some(har)).await?;
        return Ok(json!({
            "session_id": session_id,
            "contract": result.get("contract"),
            "instructions": "HAR ingested directly; no further browser workflow needed.",
        }));
    }

    Ok(json!({
        "session_id": session_id,
        "instructions": "Perform the website workflow in the active browser tab, then call api_capture_stop.",
    }))
}

async fn api_capture_stop(
    state: &AppState,
    request: &ExecuteToolRequest,
    user_id: &str,
) -> Result<Value, String> {
    let session_id = request
        .args
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'session_id' argument")?;

    // Ownership check: the session must belong to the caller.
    let session = crate::har_api_service::get_capture_session(&state.db, session_id)?
        .ok_or_else(|| format!("Capture session not found: {}", session_id))?;
    if session.user_id != user_id {
        return Err("Unauthorized".to_string());
    }

    let har = request.args.get("har").and_then(|v| v.as_str());
    crate::har_api_service::stop_capture_session(&state.db, session_id, har).await
}

async fn api_capture_replay(
    state: &AppState,
    request: &ExecuteToolRequest,
    user_id: &str,
) -> Result<Value, String> {
    let contract_id = request
        .args
        .get("contract_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'contract_id' argument")?;
    let endpoint_id = request
        .args
        .get("endpoint_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'endpoint_id' argument")?;

    // Ownership check: the endpoint must belong to a contract owned by the user.
    let contract = crate::har_api_service::get_contract_with_endpoints(&state.db, contract_id)?
        .ok_or_else(|| format!("Contract not found: {}", contract_id))?;
    if contract.0.user_id != user_id {
        return Err("Unauthorized".to_string());
    }
    if !contract.1.iter().any(|e| e.id == endpoint_id) {
        return Err(format!("Endpoint not found in contract: {}", endpoint_id));
    }

    let path_params = request.args.get("path_params");
    let query_params = request.args.get("query_params");
    let headers = request.args.get("headers");
    let body = request.args.get("body");

    crate::har_api_service::replay_endpoint(
        &state.db,
        contract_id,
        endpoint_id,
        path_params,
        query_params,
        headers,
        body,
    )
    .await
}

// ─── List tools ─────────────────────────────────────────────────────────────

async fn list_tools() -> impl IntoResponse {
    let mut tools = vec![
        json!({ "id": "shell.exec", "name": "Shell Execute", "description": "Execute shell commands" }),
        json!({ "id": "bash", "name": "Bash", "description": "Execute a shell command with optional timeout and restart control" }),
        json!({ "id": "code_execution", "name": "Code Execution", "description": "Execute code in a sandboxed environment" }),
        json!({ "id": "file.read", "name": "File Read", "description": "Read file contents" }),
        json!({ "id": "file.write", "name": "File Write", "description": "Write to a file" }),
        json!({ "id": "file.list", "name": "File List", "description": "List directory contents" }),
        json!({ "id": "http.get", "name": "HTTP GET", "description": "Make HTTP GET requests" }),
        json!({ "id": "http.post", "name": "HTTP POST", "description": "Make HTTP POST requests" }),
        json!({ "id": "system.info", "name": "System Info", "description": "Get system information" }),
        json!({ "id": "echo", "name": "Echo", "description": "Echo back arguments" }),
        json!({
            "id": "api_capture_record",
            "name": "API Capture Record",
            "description": "Start a HAR-derived API capture session. Perform the workflow in the browser, then call api_capture_stop.",
            "parameters": {
                "type": "object",
                "properties": {
                    "domain": { "type": "string", "description": "Optional domain being captured" },
                    "source": { "type": "string", "description": "Optional capture source (desktop, extension, upload)" },
                    "har": { "type": "string", "description": "Optional HAR JSON to ingest directly instead of capturing live" }
                }
            }
        }),
        json!({
            "id": "api_capture_stop",
            "name": "API Capture Stop",
            "description": "Stop a capture session and persist the derived API contract. Optionally supply a HAR JSON string.",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "The capture session ID returned by api_capture_record" },
                    "har": { "type": "string", "description": "Optional HAR JSON to derive endpoints from" }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "id": "api_capture_replay",
            "name": "API Capture Replay",
            "description": "Replay a captured endpoint server-side with supplied path, query, header, and body parameters.",
            "parameters": {
                "type": "object",
                "properties": {
                    "contract_id": { "type": "string", "description": "The contract ID containing the endpoint" },
                    "endpoint_id": { "type": "string", "description": "The endpoint ID to replay" },
                    "path_params": { "type": "object", "description": "Path parameter substitutions" },
                    "query_params": { "type": "object", "description": "Query parameter substitutions" },
                    "headers": { "type": "object", "description": "Header overrides" },
                    "body": { "type": "object", "description": "Request body JSON" }
                },
                "required": ["contract_id", "endpoint_id"]
            }
        }),
        json!({
            "id": "computer_screenshot",
            "name": "Computer Screenshot",
            "description": "Capture a screenshot of a computer (alias of the computer toolset member screenshot; the reply gives the frame size clicks are measured in).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer to screenshot" }
                },
                "required": ["computer_id"]
            }
        }),
        json!({
            "id": "computer_mouse",
            "name": "Computer Mouse",
            "description": "Move, click, drag or scroll the mouse on a computer (alias over the computer toolset; coordinates are in the last screenshot frame).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer to control" },
                    "action": { "type": "string", "enum": ["move", "click", "rightclick", "doubleclick", "mousedown", "mouseup", "drag", "scroll"], "description": "Mouse action" },
                    "x": { "type": "integer", "description": "X coordinate" },
                    "y": { "type": "integer", "description": "Y coordinate" },
                    "end_x": { "type": "integer", "description": "Drag end X" },
                    "end_y": { "type": "integer", "description": "Drag end Y" },
                    "amount": { "type": "integer", "description": "Scroll clicks (default 3)" },
                    "button": { "type": "string", "enum": ["left", "middle", "right", "up", "down"], "description": "Mouse button; up or down for scroll" }
                },
                "required": ["computer_id", "action"]
            }
        }),
        json!({
            "id": "computer_keyboard",
            "name": "Computer Keyboard",
            "description": "Type text or press a key on a computer (alias of the computer toolset members type and key).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer to control" },
                    "action": { "type": "string", "enum": ["type", "key"], "description": "Keyboard action" },
                    "text": { "type": "string", "description": "Text to type when action is type" },
                    "key": { "type": "string", "description": "Key to press when action is key (e.g. Return, Control_L, F5)" }
                },
                "required": ["computer_id", "action"]
            }
        }),
        json!({
            "id": "computer_shell",
            "name": "Computer Shell",
            "description": "Run a shell command inside the bot's cloud desktop computer.",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer" },
                    "command": { "type": "array", "items": { "type": "string" }, "description": "Command and arguments" },
                    "env": { "type": "object", "description": "Additional environment variables" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds" }
                },
                "required": ["computer_id", "command"]
            }
        }),
        json!({
            "id": "computer_file_read",
            "name": "Computer File Read",
            "description": "Read a file from the bot's cloud desktop computer (returned as base64).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer" },
                    "path": { "type": "string", "description": "Absolute guest path to the file" }
                },
                "required": ["computer_id", "path"]
            }
        }),
        json!({
            "id": "computer_file_write",
            "name": "Computer File Write",
            "description": "Write a file to the bot's cloud desktop computer (content must be base64).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer" },
                    "path": { "type": "string", "description": "Absolute guest path to write" },
                    "content": { "type": "string", "description": "Base64-encoded file content" }
                },
                "required": ["computer_id", "path", "content"]
            }
        }),
        json!({
            "id": "computer_create",
            "name": "Computer Create",
            "description": "Create a standalone cloud_desktop or local computer (ACI-approval-gated; pass approval_id from the confirmation handoff).",
            "parameters": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["cloud_desktop", "local"], "description": "Computer kind" },
                    "owner_type": { "type": "string", "enum": ["user", "org", "bot", "session"], "description": "Owner kind (default: bot, or session when session_id is set without bot_id)" },
                    "owner_id": { "type": "string", "description": "Must match the authenticated owner" },
                    "bot_id": { "type": "string", "description": "Required for bot-owned cloud_desktops" },
                    "session_id": { "type": "string", "description": "Bind the computer to a session" },
                    "name": { "type": "string" },
                    "os": { "type": "string", "description": "e.g. linux, windows, macos" },
                    "cpu_cores": { "type": "integer", "enum": [2, 4, 8] },
                    "memory_mb": { "type": "integer", "enum": [4096, 8192, 16384, 32768, 65536] },
                    "disk_mb": { "type": "integer", "enum": [20480, 40960, 81920] },
                    "resolution": { "type": "string", "enum": ["1280x720", "1920x1080", "2560x1440"] },
                    "template_id": { "type": "string", "description": "Desktop template id (exactly one of template_id/template_ref)" },
                    "template_ref": { "type": "string", "description": "Curated system/… template ref" },
                    "persistence": { "type": "string", "enum": ["ephemeral", "session", "persistent"] },
                    "provider": { "type": "string", "description": "Substrate hint: incus or tart" },
                    "approval_id": { "type": "string", "description": "Optional action-hash grant (ACI approval id) for this risky action" }
                },
                "required": ["kind"]
            }
        }),
        json!({
            "id": "computer_list",
            "name": "Computer List",
            "description": "List computers visible to the caller (read-only).",
            "parameters": {
                "type": "object",
                "properties": {
                    "bot_id": { "type": "string", "description": "Filter by owning bot" },
                    "kind": { "type": "string", "enum": ["local", "byo_vps", "managed", "byoc", "cloud_desktop"] },
                    "group_id": { "type": "string", "description": "Filter by computer group" },
                    "include_roles": { "type": "string", "description": "Truthy values include non-user roles (golden template-build holders)" }
                }
            }
        }),
        json!({
            "id": "computer_start",
            "name": "Computer Start",
            "description": "Start (resume) a stopped cloud_desktop or local computer (ACI-approval-gated).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer to start" },
                    "approval_id": { "type": "string", "description": "Optional action-hash grant (ACI approval id) for this risky action" }
                },
                "required": ["computer_id"]
            }
        }),
        json!({
            "id": "computer_stop",
            "name": "Computer Stop",
            "description": "Stop (pause) a cloud_desktop or local computer (ACI-approval-gated).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer to stop" },
                    "approval_id": { "type": "string", "description": "Optional action-hash grant (ACI approval id) for this risky action" }
                },
                "required": ["computer_id"]
            }
        }),
        json!({
            "id": "computer_resize",
            "name": "Computer Resize",
            "description": "Resize CPU/memory/disk of a cloud_desktop computer (ACI-approval-gated; disk resize requires a stopped computer).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer" },
                    "cpu_cores": { "type": "integer", "enum": [2, 4, 8] },
                    "memory_mb": { "type": "integer", "enum": [4096, 8192, 16384, 32768, 65536] },
                    "disk_mb": { "type": "integer", "enum": [20480, 40960, 81920] },
                    "approval_id": { "type": "string", "description": "Optional action-hash grant (ACI approval id) for this risky action" }
                },
                "required": ["computer_id"]
            }
        }),
        json!({
            "id": "computer_clone",
            "name": "Computer Clone",
            "description": "Clone a running or stopped cloud_desktop into a new computer (ACI-approval-gated).",
            "parameters": {
                "type": "object",
                "properties": {
                    "computer_id": { "type": "string", "description": "ID of the computer to clone" },
                    "name": { "type": "string", "description": "Name for the clone (default: \"<source> (copy)\")" },
                    "approval_id": { "type": "string", "description": "Optional action-hash grant (ACI approval id) for this risky action" }
                },
                "required": ["computer_id"]
            }
        }),
    ];
    for tool in markdown_builtin_tools() {
        tools.push(json!({
            "id": tool.get("id").cloned().unwrap_or(Value::Null),
            "name": tool.get("label").cloned().unwrap_or(Value::Null),
            "description": tool.get("description").cloned().unwrap_or(Value::Null),
            "parameters": tool.get("parameters").cloned().unwrap_or(json!({})),
        }));
    }
    let total = tools.len();
    Json(json!({
        "tools": tools,
        // The web/desktop tool registry (`tool-registry.store.ts`,
        // `native-agent-api.ts`) reads grouped buckets — mirror the same list
        // under `native` so those consumers actually see the registry
        // (previously they read a missing key and rendered zero tools).
        "native": tools,
        "total": total,
    }))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission_policy::{PermissionPolicy, PermissionRule};

    #[test]
    fn truncate_markdown_leaves_short_output_untouched() {
        let input = "# Hello\n\nSome markdown.";
        let (out, truncated) = truncate_markdown(input);
        assert_eq!(out, input);
        assert!(!truncated);
    }

    #[test]
    fn truncate_markdown_caps_long_output_with_note() {
        let input = "a".repeat(MAX_TOOL_MARKDOWN_CHARS + 500);
        let (out, truncated) = truncate_markdown(&input);
        assert!(truncated);
        assert!(out.starts_with(&"a".repeat(1000)));
        assert!(out.contains("Output truncated"));
        assert!(out.contains(&(MAX_TOOL_MARKDOWN_CHARS + 500).to_string()));
    }

    #[test]
    fn truncate_markdown_respects_char_boundaries() {
        // Multibyte characters must never be split mid-codepoint.
        let input = "é".repeat(MAX_TOOL_MARKDOWN_CHARS + 10);
        let (out, truncated) = truncate_markdown(&input);
        assert!(truncated);
        let head = out.split("\n\n_").next().unwrap();
        assert_eq!(head.chars().count(), MAX_TOOL_MARKDOWN_CHARS);
    }

    #[test]
    fn markdown_tool_result_errors_without_markdown_field() {
        let body = json!({"title": "nope"});
        assert!(markdown_tool_result(&body).is_err());
    }

    #[test]
    fn markdown_tool_result_maps_engine_payload() {
        let body = json!({
            "markdown": "# Title\n\nbody",
            "title": "Title",
            "format": "docx",
            "sourceUrl": "https://example.com",
        });
        let result = markdown_tool_result(&body).unwrap();
        assert_eq!(result["markdown"], "# Title\n\nbody");
        assert_eq!(result["truncated"], false);
        assert_eq!(result["title"], "Title");
        assert_eq!(result["format"], "docx");
        assert_eq!(result["sourceUrl"], "https://example.com");
    }

    #[test]
    fn markdown_tool_result_truncates_engine_payload() {
        let body = json!({ "markdown": "b".repeat(MAX_TOOL_MARKDOWN_CHARS * 2) });
        let result = markdown_tool_result(&body).unwrap();
        assert_eq!(result["truncated"], true);
        assert!(result["markdown"]
            .as_str()
            .unwrap()
            .contains("Output truncated"));
    }

    #[test]
    fn execute_tool_request_accepts_surface_field_names() {
        // The web/desktop surface posts `tool_name`/`parameters`.
        let req: ExecuteToolRequest = serde_json::from_str(
            r#"{"tool_name": "echo", "parameters": {"message": "hi"}, "session_id": "s1"}"#,
        )
        .unwrap();
        assert_eq!(req.tool, "echo");
        assert_eq!(req.args["message"], "hi");

        // And the canonical `tool`/`args` keys keep working.
        let req: ExecuteToolRequest =
            serde_json::from_str(r#"{"tool": "echo", "args": {}}"#).unwrap();
        assert_eq!(req.tool, "echo");
    }

    #[test]
    fn markdown_builtin_tools_advertise_expected_contract() {
        let tools = markdown_builtin_tools();
        assert_eq!(tools.len(), 2);

        let doc = &tools[0];
        assert_eq!(doc["id"], "document_to_markdown");
        assert!(doc["description"].as_str().unwrap().len() > 20);
        let props = doc["parameters"]["properties"].as_object().unwrap();
        assert!(props.contains_key("uploadId"));
        assert!(props.contains_key("artifactId"));

        let url = &tools[1];
        assert_eq!(url["id"], "url_to_markdown");
        assert_eq!(
            url["parameters"]["required"],
            json!(["url"])
        );
    }

    #[tokio::test]
    async fn bash_tool_executes_shell_command() {
        let req = ExecuteToolRequest {
            tool: "bash".to_string(),
            args: json!({ "command": "echo hello" }),
            timeout: Some(5),
            workspace_id: None,
            ..Default::default()
        };
        let result = bash_exec(&req).await.unwrap();
        assert!(result["success"].as_bool().unwrap());
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn bash_tool_honors_timeout_argument() {
        let req = ExecuteToolRequest {
            tool: "bash".to_string(),
            args: json!({ "command": "sleep 5", "timeout": 1 }),
            timeout: Some(30),
            workspace_id: None,
            ..Default::default()
        };
        let result = bash_exec(&req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn code_execution_runs_python() {
        let req = ExecuteToolRequest {
            tool: "code_execution".to_string(),
            args: json!({ "language": "python", "code": "print(21 + 21)" }),
            timeout: Some(5),
            workspace_id: None,
            ..Default::default()
        };
        let result = code_execution(&req).await.unwrap();
        assert!(result["success"].as_bool().unwrap());
        assert!(result["stdout"].as_str().unwrap().contains("42"));
        assert_eq!(result["language"], "python");
    }

    #[tokio::test]
    async fn code_execution_rejects_missing_language() {
        let req = ExecuteToolRequest {
            tool: "code_execution".to_string(),
            args: json!({ "code": "print(1)" }),
            timeout: Some(5),
            workspace_id: None,
            ..Default::default()
        };
        let result = code_execution(&req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("language"));
    }

    #[test]
    fn tool_result_cache_control_attaches_to_large_results_when_enabled() {
        let large_content = "x".repeat(TOOL_RESULT_CACHE_THRESHOLD_CHARS);
        let result = json!({ "content": large_content });
        let cache = tool_result_cache_control(&result, true);
        assert_eq!(cache, Some(json!({"type": "ephemeral"})));
    }

    #[test]
    fn tool_result_cache_control_omitted_when_caching_disabled() {
        let large_content = "x".repeat(TOOL_RESULT_CACHE_THRESHOLD_CHARS);
        let result = json!({ "content": large_content });
        let cache = tool_result_cache_control(&result, false);
        assert_eq!(cache, None);
    }

    #[test]
    fn tool_result_cache_control_omitted_for_small_results() {
        let result = json!({ "content": "small" });
        let cache = tool_result_cache_control(&result, true);
        assert_eq!(cache, None);
    }

    async fn state_with_policy(policy: PermissionPolicy, active: &str) -> Arc<AppState> {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = crate::test_helpers::app_state(&temp).await;
        let mut state = match Arc::try_unwrap(state) {
            Ok(s) => s,
            Err(_) => panic!("single app_state reference expected"),
        };
        state.config.user.permission_policies = Some(vec![policy]);
        state.config.user.active_permission_policy = Some(active.to_string());
        // Tests exercise host-control tools through the stateful router, so
        // enable host control for the test fixture.
        state.config.user.host_control_enabled = Some(true);
        Arc::new(state)
    }

    fn user_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-allternit-user-id", "user-1".parse().unwrap());
        headers
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn execute_tool_allows_when_policy_allows() {
        let state = state_with_policy(
            PermissionPolicy {
                name: "allow-echo".to_string(),
                rules: vec![PermissionRule {
                    tool: Some("echo".to_string()),
                    action: PermissionAction::Allow,
                    ..Default::default()
                }],
            },
            "allow-echo",
        )
        .await;
        let body = ExecuteToolRequest {
            tool: "echo".to_string(),
            args: json!({ "message": "hi" }),
            ..Default::default()
        };
        let response = execute_tool(State(state), user_headers(), Json(body)).await;
        assert_eq!(response.into_response().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn execute_tool_denies_when_policy_denies() {
        let state = state_with_policy(
            PermissionPolicy {
                name: "deny-bash".to_string(),
                rules: vec![PermissionRule {
                    tool: Some("bash".to_string()),
                    action: PermissionAction::Deny,
                    ..Default::default()
                }],
            },
            "deny-bash",
        )
        .await;
        let body = ExecuteToolRequest {
            tool: "bash".to_string(),
            args: json!({ "command": "echo hi" }),
            ..Default::default()
        };
        let response = execute_tool(State(state), user_headers(), Json(body)).await.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let json = response_json(response).await;
        assert_eq!(json["tool"], "bash");
    }

    #[tokio::test]
    async fn execute_tool_asks_when_policy_asks() {
        let state = state_with_policy(
            PermissionPolicy {
                name: "ask-write".to_string(),
                rules: vec![PermissionRule {
                    tool: Some("file.write".to_string()),
                    file_path: Some("/etc/*".to_string()),
                    action: PermissionAction::Ask,
                    ..Default::default()
                }],
            },
            "ask-write",
        )
        .await;
        let body = ExecuteToolRequest {
            tool: "file.write".to_string(),
            args: json!({ "path": "/etc/test.txt", "content": "x" }),
            ..Default::default()
        };
        let response = execute_tool(State(state.clone()), user_headers(), Json(body))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let json = response_json(response).await;
        let approval_id = json["approval_id"].as_str().unwrap();
        assert_eq!(json["status"], "approval_required");

        // Approve the request and verify the store reflects it.
        let approve_response = approve_tool_execution(State(state.clone()), Path(approval_id.to_string())).await;
        assert_eq!(approve_response.into_response().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn approval_endpoints_return_404_for_unknown_id() {
        let state = state_with_policy(
            PermissionPolicy {
                name: "empty".to_string(),
                rules: vec![],
            },
            "empty",
        )
        .await;
        let response = approve_tool_execution(State(state.clone()), Path("nope".to_string())).await;
        assert_eq!(response.into_response().status(), StatusCode::NOT_FOUND);
        let response = deny_tool_execution(State(state), Path("nope".to_string())).await;
        assert_eq!(response.into_response().status(), StatusCode::NOT_FOUND);
    }
}
