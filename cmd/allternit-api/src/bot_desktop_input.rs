//! Bot desktop control endpoints (mouse, keyboard, shell, file transfer).
//!
//! These handlers are kept in a separate module so `bot_desktop_routes.rs`
//! stays under the 1,500 LOC feature limit.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
#[cfg(test)]
use axum::{routing::get, routing::post, Router};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::bot_desktop_routes::{
    build_handle, read_bot_sandbox, verify_bot_ownership, DesktopQuery,
};
use crate::bot_desktop_windows;
use crate::AppState;
use allternit_driver_interface::CommandSpec;

#[cfg(test)]
pub fn bot_desktop_input_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bots/:bot_id/desktop/mouse", post(send_desktop_mouse))
        .route(
            "/bots/:bot_id/desktop/keyboard",
            post(send_desktop_keyboard),
        )
        .route("/bots/:bot_id/desktop/shell", post(run_desktop_shell))
        .route(
            "/bots/:bot_id/desktop/files/download",
            get(download_desktop_file),
        )
        .route(
            "/bots/:bot_id/desktop/files/upload",
            post(upload_desktop_file),
        )
}

#[derive(Debug, Clone, Deserialize)]
pub struct MouseInput {
    /// Action to perform: `move`, `click`, `rightclick`, `doubleclick`,
    /// `mousedown`, or `mouseup`.
    pub action: String,
    /// X coordinate on the virtual screen.
    pub x: Option<i32>,
    /// Y coordinate on the virtual screen.
    pub y: Option<i32>,
    /// Mouse button: `left`, `middle`, or `right`. Defaults to `left`.
    pub button: Option<String>,
    pub end_x: Option<i32>,
    pub end_y: Option<i32>,
    pub amount: Option<i32>,
}

pub(crate) fn desktop_display(provider: &str) -> &'static str {
    match provider {
        "tart" => ":99",
        _ => ":0",
    }
}

pub(crate) async fn send_desktop_mouse(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Query(query): Query<DesktopQuery>,
    Json(input): Json<MouseInput>,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    let _driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let sandbox_id = match crate::bot_desktop_routes::resolve_sandbox_id(&state, &bot_id, &query).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let record = match read_bot_sandbox(&state.db, &bot_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "bot has no desktop sandbox"})),
            )
                .into_response();
        }
        Err(e) => {
            warn!(bot_id, error = %e, "Failed to read bot sandbox");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to read sandbox record"})),
            )
                .into_response();
        }
    };
    send_desktop_mouse_core(
        &state,
        &sandbox_id,
        &record.os,
        &record.provider,
        Some(&bot_id),
        input,
    )
    .await
}

pub(crate) async fn send_desktop_mouse_core(
    state: &Arc<AppState>,
    sandbox_id: &str,
    os: &str,
    provider: &str,
    bot_id: Option<&str>,
    input: MouseInput,
) -> axum::response::Response {
    if os == "windows" && matches!(input.action.to_lowercase().as_str(), "drag" | "scroll") {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({"error": "windows input path supports move and click only"})),
        )
            .into_response();
    }
    let driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let command = if os == "windows" {
        match build_windows_mouse_command(&input) {
            Ok(cmd) => cmd,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": err}))).into_response();
            }
        }
    } else {
        match build_mouse_command(&input, desktop_display(provider)) {
            Ok(cmd) => cmd,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": err}))).into_response();
            }
        }
    };

    run_guest_command(&*driver, sandbox_id, os, provider, command, "mouse", bot_id).await
}

pub(crate) fn build_mouse_command(
    input: &MouseInput,
    display: &str,
) -> Result<Vec<String>, String> {
    let action = input.action.to_lowercase();
    if !matches!(
        action.as_str(),
        "move"
            | "click"
            | "rightclick"
            | "doubleclick"
            | "mousedown"
            | "mouseup"
            | "drag"
            | "scroll"
    ) {
        return Err("valid mouse actions: move, click, rightclick, doubleclick, mousedown, mouseup, drag, scroll".into());
    }
    if action == "scroll" {
        let button = match input.button.as_deref().unwrap_or("down") {
            "up" => "4",
            "down" => "5",
            _ => return Err("scroll button must be up or down".into()),
        };
        let amount = input.amount.unwrap_or(3);
        if amount <= 0 {
            return Err("scroll amount must be positive".into());
        }
        return Ok(vec![
            "env".into(),
            format!("DISPLAY={display}"),
            "xdotool".into(),
            "click".into(),
            "--repeat".into(),
            amount.to_string(),
            button.into(),
        ]);
    }
    if action == "drag" {
        let (Some(x), Some(y), Some(ex), Some(ey)) = (input.x, input.y, input.end_x, input.end_y)
        else {
            return Err("drag requires x, y, end_x, end_y".into());
        };
        let mut cmd = vec![
            "env".into(),
            format!("DISPLAY={display}"),
            "xdotool".into(),
            "mousemove".into(),
            x.to_string(),
            y.to_string(),
            "mousedown".into(),
            "1".into(),
        ];
        for step in 1..=10i64 {
            cmd.extend([
                "mousemove".into(),
                "--sync".into(),
                (x as i64 + (ex as i64 - x as i64) * step / 10).to_string(),
                (y as i64 + (ey as i64 - y as i64) * step / 10).to_string(),
            ]);
        }
        cmd.extend(["mouseup".into(), "1".into()]);
        return Ok(cmd);
    }
    let button_num = match input
        .button
        .as_deref()
        .unwrap_or("left")
        .to_lowercase()
        .as_str()
    {
        "left" => "1",
        "middle" => "2",
        "right" => "3",
        other => return Err(format!("unsupported button: {}", other)),
    };

    let x = input.x.unwrap_or(0);
    let y = input.y.unwrap_or(0);

    let args = match action.as_str() {
        "move" => vec!["mousemove".to_string(), x.to_string(), y.to_string()],
        "click" => vec![
            "mousemove".to_string(),
            x.to_string(),
            y.to_string(),
            "click".to_string(),
            button_num.to_string(),
        ],
        "rightclick" => vec![
            "mousemove".to_string(),
            x.to_string(),
            y.to_string(),
            "click".to_string(),
            "3".to_string(),
        ],
        "doubleclick" => vec![
            "mousemove".to_string(),
            x.to_string(),
            y.to_string(),
            "click".to_string(),
            "--repeat".to_string(),
            "2".to_string(),
            "--delay".to_string(),
            "50".to_string(),
            button_num.to_string(),
        ],
        "mousedown" => vec![
            "mousemove".to_string(),
            x.to_string(),
            y.to_string(),
            "mousedown".to_string(),
            button_num.to_string(),
        ],
        "mouseup" => vec![
            "mousemove".to_string(),
            x.to_string(),
            y.to_string(),
            "mouseup".to_string(),
            button_num.to_string(),
        ],
        other => return Err(format!("unsupported mouse action: {}", other)),
    };

    Ok(vec![
        "env".to_string(),
        format!("DISPLAY={}", display),
        "xdotool".to_string(),
    ]
    .into_iter()
    .chain(args)
    .collect())
}

pub(crate) fn build_windows_mouse_command(input: &MouseInput) -> Result<Vec<String>, String> {
    let action = input.action.to_lowercase();
    if !matches!(
        action.as_str(),
        "move"
            | "click"
            | "rightclick"
            | "doubleclick"
            | "mousedown"
            | "mouseup"
            | "drag"
            | "scroll"
    ) {
        return Err("valid mouse actions: move, click, rightclick, doubleclick, mousedown, mouseup, drag, scroll".into());
    }
    let x = input.x.unwrap_or(0);
    let y = input.y.unwrap_or(0);
    let cmd = match action.as_str() {
        "move" => bot_desktop_windows::mouse_move_command(x, y).command,
        "click" => {
            let mut c = bot_desktop_windows::mouse_move_command(x, y).command;
            c.extend(bot_desktop_windows::mouse_click_command().command);
            c
        }
        other => return Err(format!("unsupported windows mouse action: {}", other)),
    };
    Ok(cmd)
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyboardInput {
    /// Action to perform: `type` or `key`.
    pub action: String,
    /// Text to type when `action` is `type`.
    pub text: Option<String>,
    /// Key to press when `action` is `key` (e.g. `Return`, `Control_L`, `F5`).
    pub key: Option<String>,
}

pub(crate) async fn send_desktop_keyboard(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Query(query): Query<DesktopQuery>,
    Json(input): Json<KeyboardInput>,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    let _driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let sandbox_id = match crate::bot_desktop_routes::resolve_sandbox_id(&state, &bot_id, &query).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let record = match read_bot_sandbox(&state.db, &bot_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "bot has no desktop sandbox"})),
            )
                .into_response();
        }
        Err(e) => {
            warn!(bot_id, error = %e, "Failed to read bot sandbox");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to read sandbox record"})),
            )
                .into_response();
        }
    };
    send_desktop_keyboard_core(
        &state,
        &sandbox_id,
        &record.os,
        &record.provider,
        Some(&bot_id),
        input,
    )
    .await
}

pub(crate) async fn send_desktop_keyboard_core(
    state: &Arc<AppState>,
    sandbox_id: &str,
    os: &str,
    provider: &str,
    bot_id: Option<&str>,
    input: KeyboardInput,
) -> axum::response::Response {
    let driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let command = if os == "windows" {
        match build_windows_keyboard_command(&input) {
            Ok(cmd) => cmd,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": err}))).into_response();
            }
        }
    } else {
        match build_keyboard_command(&input, desktop_display(provider)) {
            Ok(cmd) => cmd,
            Err(err) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": err}))).into_response();
            }
        }
    };

    run_guest_command(
        &*driver, sandbox_id, os, provider, command, "keyboard", bot_id,
    )
    .await
}

pub(crate) fn build_keyboard_command(
    input: &KeyboardInput,
    display: &str,
) -> Result<Vec<String>, String> {
    let action = input.action.to_lowercase();
    match action.as_str() {
        "type" => {
            let text = input
                .text
                .as_deref()
                .ok_or_else(|| "text is required for action=type".to_string())?;
            Ok(vec![
                "env".to_string(),
                format!("DISPLAY={}", display),
                "xdotool".to_string(),
                "type".to_string(),
                "--delay".to_string(),
                "10".to_string(),
                text.to_string(),
            ])
        }
        "key" => {
            let key = input
                .key
                .as_deref()
                .ok_or_else(|| "key is required for action=key".to_string())?;
            Ok(vec![
                "env".to_string(),
                format!("DISPLAY={}", display),
                "xdotool".to_string(),
                "key".to_string(),
                key.to_string(),
            ])
        }
        other => Err(format!("unsupported keyboard action: {}", other)),
    }
}

pub(crate) fn build_windows_keyboard_command(input: &KeyboardInput) -> Result<Vec<String>, String> {
    let action = input.action.to_lowercase();
    match action.as_str() {
        "type" => {
            let text = input
                .text
                .as_deref()
                .ok_or_else(|| "text is required for action=type".to_string())?;
            Ok(bot_desktop_windows::keyboard_type_command(text).command)
        }
        other => Err(format!("unsupported windows keyboard action: {}", other)),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShellInput {
    /// Command and arguments to execute inside the guest.
    pub command: Vec<String>,
    /// Additional environment variables for the command.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional timeout in seconds (ignored by the Incus substrate today;
    /// the guest command is run synchronously with a 60 s wait).
    #[serde(default)]
    pub timeout: Option<u64>,
}

pub(crate) async fn run_desktop_shell(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Query(_query): Query<DesktopQuery>,
    Json(input): Json<ShellInput>,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    if input.command.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "command must not be empty"})),
        )
            .into_response();
    }

    let _driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let record = match read_bot_sandbox(&state.db, &bot_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "bot has no desktop sandbox"})),
            )
                .into_response();
        }
        Err(e) => {
            warn!(bot_id, error = %e, "Failed to read bot sandbox");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to read sandbox record"})),
            )
                .into_response();
        }
    };
    run_desktop_shell_core(
        &state,
        &record.sandbox_id,
        &record.os,
        &record.provider,
        Some(&bot_id),
        input,
    )
    .await
}

pub(crate) async fn run_desktop_shell_core(
    state: &Arc<AppState>,
    sandbox_id: &str,
    os: &str,
    provider: &str,
    bot_id: Option<&str>,
    input: ShellInput,
) -> axum::response::Response {
    if input.command.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "command must not be empty"})),
        )
            .into_response();
    }
    let driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let handle = build_handle(sandbox_id, Some(os), Some(provider));
    let cmd_spec = if os == "windows" {
        bot_desktop_windows::shell_command(&input.command.join(" "))
    } else {
        let mut env_vars = HashMap::new();
        env_vars.insert("DISPLAY".to_string(), desktop_display(provider).to_string());
        env_vars.extend(input.env);
        // Inline DISPLAY (and any other env vars) into the command so the
        // execution works even with drivers that do not transmit env_vars.
        let mut command = vec!["env".to_string()];
        for (k, v) in &env_vars {
            command.push(format!("{}={}", k, v));
        }
        command.extend(input.command);
        CommandSpec {
            command,
            env_vars,
            working_dir: None,
            stdin_data: None,
            capture_stdout: true,
            capture_stderr: true,
        }
    };

    match driver.exec(&handle, cmd_spec).await {
        Ok(result) => {
            let stdout = String::from_utf8_lossy(result.stdout.as_deref().unwrap_or(&[]));
            let stderr = String::from_utf8_lossy(result.stderr.as_deref().unwrap_or(&[]));
            (
                StatusCode::OK,
                Json(json!({
                    "exit_code": result.exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                    "duration_ms": result.duration_ms,
                })),
            )
                .into_response()
        }
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, error = %e, "Failed to run desktop shell");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to run shell: {}", e)})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilePathQuery {
    /// Absolute guest path to the file.
    pub path: String,
}

pub(crate) async fn download_desktop_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Query(query): Query<DesktopQuery>,
    Query(file_query): Query<FilePathQuery>,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    let _driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let sandbox_id = match crate::bot_desktop_routes::resolve_sandbox_id(&state, &bot_id, &query).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let record = match read_bot_sandbox(&state.db, &bot_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "bot has no desktop sandbox"})),
            )
                .into_response();
        }
        Err(e) => {
            warn!(bot_id, error = %e, "Failed to read bot sandbox");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to read sandbox record"})),
            )
                .into_response();
        }
    };
    download_desktop_file_core(
        &state,
        &sandbox_id,
        &record.os,
        &record.provider,
        Some(&bot_id),
        file_query,
    )
    .await
}

pub(crate) async fn download_desktop_file_core(
    state: &Arc<AppState>,
    sandbox_id: &str,
    os: &str,
    provider: &str,
    bot_id: Option<&str>,
    file_query: FilePathQuery,
) -> axum::response::Response {
    // Every caller (computer routes, bot desktop routes) gets this: the path
    // is on the guest, so it's checked by its text (files_scope).
    if let Err(denied) = crate::files_scope::check_guest_path(&file_query.path) {
        return crate::files_scope::denied_response(denied);
    }
    let driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let handle = build_handle(sandbox_id, Some(os), Some(provider));
    match driver.pull_file(&handle, &file_query.path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            Bytes::from(bytes),
        )
            .into_response(),
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, path = %file_query.path, error = %e, "Failed to download desktop file");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to download file: {}", e)})),
            )
                .into_response()
        }
    }
}

pub(crate) async fn upload_desktop_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Query(query): Query<DesktopQuery>,
    Query(file_query): Query<FilePathQuery>,
    body: Bytes,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "upload body must not be empty"})),
        )
            .into_response();
    }

    let _driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let sandbox_id = match crate::bot_desktop_routes::resolve_sandbox_id(&state, &bot_id, &query).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let record = match read_bot_sandbox(&state.db, &bot_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "bot has no desktop sandbox"})),
            )
                .into_response();
        }
        Err(e) => {
            warn!(bot_id, error = %e, "Failed to read bot sandbox");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to read sandbox record"})),
            )
                .into_response();
        }
    };
    upload_desktop_file_core(
        &state,
        &sandbox_id,
        &record.os,
        &record.provider,
        Some(&bot_id),
        file_query,
        body,
    )
    .await
}

pub(crate) async fn upload_desktop_file_core(
    state: &Arc<AppState>,
    sandbox_id: &str,
    os: &str,
    provider: &str,
    bot_id: Option<&str>,
    file_query: FilePathQuery,
    body: Bytes,
) -> axum::response::Response {
    if let Err(denied) = crate::files_scope::check_guest_path(&file_query.path) {
        return crate::files_scope::denied_response(denied);
    }
    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "upload body must not be empty"})),
        )
            .into_response();
    }
    let driver = match &state.vm_driver {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "No VM driver is configured on this host"})),
            )
                .into_response();
        }
    };

    let handle = build_handle(sandbox_id, Some(os), Some(provider));
    match driver
        .push_file(&handle, &file_query.path, body.to_vec())
        .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({"success": true}))).into_response(),
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, path = %file_query.path, error = %e, "Failed to upload desktop file");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to upload file: {}", e)})),
            )
                .into_response()
        }
    }
}

async fn run_guest_command(
    driver: &dyn allternit_driver_interface::ExecutionDriver,
    sandbox_id: &str,
    os: &str,
    provider: &str,
    command: Vec<String>,
    command_kind: &str,
    bot_id: Option<&str>,
) -> axum::response::Response {
    let handle = build_handle(sandbox_id, Some(os), Some(provider));
    let mut env_vars = std::collections::HashMap::new();
    env_vars.insert("DISPLAY".to_string(), ":0".to_string());
    let cmd_spec = CommandSpec {
        command,
        env_vars,
        working_dir: None,
        stdin_data: None,
        capture_stdout: true,
        capture_stderr: true,
    };

    match driver.exec(&handle, cmd_spec).await {
        Ok(result) => {
            if result.exit_code != 0 {
                let stderr = String::from_utf8_lossy(result.stderr.as_deref().unwrap_or(&[]));
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": format!("{} command failed", command_kind),
                        "exit_code": result.exit_code,
                        "stderr": stderr.trim(),
                    })),
                )
                    .into_response();
            }
            (StatusCode::OK, Json(json!({"success": true}))).into_response()
        }
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, error = %e, "Failed to send desktop {} input", command_kind);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to send {} input: {}", command_kind, e)})),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use crate::AppState;
    use allternit_driver_interface::{ExecResult, ExecutionDriver};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    #[derive(Debug)]
    struct MockExecutionDriver {
        calls: Arc<Mutex<Vec<String>>>,
        exec_result: Arc<
            Mutex<Option<std::result::Result<ExecResult, allternit_driver_interface::DriverError>>>,
        >,
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    impl MockExecutionDriver {
        fn new() -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                exec_result: Arc::new(Mutex::new(None)),
                files: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        fn recorded(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn set_exec_result(
            &self,
            result: std::result::Result<ExecResult, allternit_driver_interface::DriverError>,
        ) {
            *self.exec_result.lock().unwrap() = Some(result);
        }

        fn seed_file(&self, path: &str, content: Vec<u8>) {
            self.files.lock().unwrap().insert(path.to_string(), content);
        }
    }

    #[async_trait::async_trait]
    impl ExecutionDriver for MockExecutionDriver {
        fn capabilities(&self) -> allternit_driver_interface::DriverCapabilities {
            use allternit_driver_interface::{
                DriverCapabilities, DriverFeatures, DriverType, EnvSpecType, IsolationLevel,
                ResourceSpec,
            };
            DriverCapabilities {
                resize: false,
                clone: false,
                driver_type: DriverType::Container,
                isolation: IsolationLevel::Standard,
                max_resources: ResourceSpec {
                    cpu_millis: 2000,
                    memory_mib: 4096,
                    disk_mib: Some(20480),
                    network_egress_kib: None,
                    gpu_count: None,
                },
                supported_env_specs: vec![EnvSpecType::Oci],
                features: DriverFeatures {
                    snapshot: false,
                    live_restore: false,
                    gpu: false,
                    prewarm: false,
                },
            }
        }

        async fn spawn(
            &self,
            _spec: allternit_driver_interface::SpawnSpec,
        ) -> std::result::Result<
            allternit_driver_interface::ExecutionHandle,
            allternit_driver_interface::DriverError,
        > {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "spawn".to_string(),
            })
        }

        async fn pause_vm(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn resume_vm(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn exec(
            &self,
            handle: &allternit_driver_interface::ExecutionHandle,
            _cmd: CommandSpec,
        ) -> std::result::Result<ExecResult, allternit_driver_interface::DriverError> {
            self.calls.lock().unwrap().push(format!(
                "exec:{}",
                handle
                    .driver_info
                    .get("native_id")
                    .unwrap_or(&"?".to_string())
            ));
            match self.exec_result.lock().unwrap().take() {
                Some(result) => result,
                None => Err(allternit_driver_interface::DriverError::NotSupported {
                    feature: "exec".to_string(),
                }),
            }
        }

        async fn stream_logs(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<
            Vec<allternit_driver_interface::LogEntry>,
            allternit_driver_interface::DriverError,
        > {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "stream_logs".to_string(),
            })
        }

        async fn get_artifacts(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<
            Vec<allternit_driver_interface::Artifact>,
            allternit_driver_interface::DriverError,
        > {
            Ok(vec![])
        }

        async fn destroy(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn get_consumption(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<
            allternit_driver_interface::ResourceConsumption,
            allternit_driver_interface::DriverError,
        > {
            Ok(allternit_driver_interface::ResourceConsumption::default())
        }

        async fn get_receipt(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<
            Option<allternit_driver_interface::Receipt>,
            allternit_driver_interface::DriverError,
        > {
            Ok(None)
        }

        async fn pull_file(
            &self,
            handle: &allternit_driver_interface::ExecutionHandle,
            path: &str,
        ) -> std::result::Result<Vec<u8>, allternit_driver_interface::DriverError> {
            self.calls.lock().unwrap().push(format!(
                "pull_file:{}:{}",
                handle
                    .driver_info
                    .get("native_id")
                    .unwrap_or(&"?".to_string()),
                path
            ));
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| allternit_driver_interface::DriverError::NotFound {
                    id: path.to_string(),
                })
        }

        async fn push_file(
            &self,
            handle: &allternit_driver_interface::ExecutionHandle,
            path: &str,
            content: Vec<u8>,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            self.calls.lock().unwrap().push(format!(
                "push_file:{}:{}",
                handle
                    .driver_info
                    .get("native_id")
                    .unwrap_or(&"?".to_string()),
                path
            ));
            self.files.lock().unwrap().insert(path.to_string(), content);
            Ok(())
        }

        async fn health_check(
            &self,
        ) -> std::result::Result<
            allternit_driver_interface::DriverHealth,
            allternit_driver_interface::DriverError,
        > {
            Ok(allternit_driver_interface::DriverHealth {
                healthy: true,
                message: Some("mock".to_string()),
                active_executions: 0,
                available_capacity: self.capabilities().max_resources,
                capabilities: vec![],
            })
        }
    }

    fn test_user(user_id: &str) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn test_app_state(temp: &Path, driver: Arc<MockExecutionDriver>) -> Arc<AppState> {
        let state = crate::test_helpers::app_state_with_driver(temp, Some(driver)).await;
        let conn = state.db.connect().expect("test db conn");
        conn.execute(
            "INSERT OR IGNORE INTO agents (id, user_id, name, type, model, provider)
             VALUES (?1, ?2, 'Test Bot', 'worker', 'gpt-4', 'openai')",
            rusqlite::params!["bot-1", "user-1"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO bot_desktop_sandboxes (bot_id, sandbox_id, provider, host, status, os)
             VALUES (?1, ?2, 'incus', 'mail', 'running', 'linux')",
            rusqlite::params!["bot-1", "sandbox-abc"],
        )
        .unwrap();
        drop(conn);
        state
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::Value::Null)
    }

    #[tokio::test]
    async fn mouse_endpoint_sends_xdotool_click() {
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(ExecResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            duration_ms: 10,
            resource_usage: allternit_driver_interface::ResourceConsumption::default(),
        }));
        let state = test_app_state(&temp, driver.clone()).await;
        let app = bot_desktop_input_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bots/bot-1/desktop/mouse?sandbox_id=sandbox-abc")
                    .extension(test_user("user-1"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"action":"click","x":100,"y":200}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let calls = driver.recorded();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("exec:sandbox-abc"));
    }

    #[tokio::test]
    async fn keyboard_endpoint_sends_xdotool_type() {
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(ExecResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            duration_ms: 10,
            resource_usage: allternit_driver_interface::ResourceConsumption::default(),
        }));
        let state = test_app_state(&temp, driver.clone()).await;
        let app = bot_desktop_input_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bots/bot-1/desktop/keyboard?sandbox_id=sandbox-abc")
                    .extension(test_user("user-1"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"action":"type","text":"hello world"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let calls = driver.recorded();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("exec:sandbox-abc"));
    }

    #[tokio::test]
    async fn mouse_command_builder_rejects_unknown_action() {
        let input = MouseInput {
            action: "hover".to_string(),
            x: Some(0),
            y: Some(0),
            button: None,
            end_x: None,
            end_y: None,
            amount: None,
        };
        assert!(build_mouse_command(&input, ":0").is_err());
    }

    #[tokio::test]
    async fn keyboard_command_builder_rejects_missing_text() {
        let input = KeyboardInput {
            action: "type".to_string(),
            text: None,
            key: None,
        };
        assert!(build_keyboard_command(&input, ":0").is_err());
    }

    #[tokio::test]
    async fn shell_endpoint_returns_command_output() {
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(ExecResult {
            exit_code: 0,
            stdout: Some(b"hello stdout".to_vec()),
            stderr: Some(b"hello stderr".to_vec()),
            duration_ms: 42,
            resource_usage: allternit_driver_interface::ResourceConsumption::default(),
        }));
        let state = test_app_state(&temp, driver.clone()).await;
        let app = bot_desktop_input_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bots/bot-1/desktop/shell?sandbox_id=sandbox-abc")
                    .extension(test_user("user-1"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"command":["echo","hello"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response.into_body()).await;
        assert_eq!(body["exit_code"], 0);
        assert_eq!(body["stdout"], "hello stdout");
        assert_eq!(body["stderr"], "hello stderr");
        assert_eq!(body["duration_ms"], 42);
    }

    #[tokio::test]
    async fn download_endpoint_returns_file_bytes() {
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.seed_file("/tmp/test.txt", b"file contents".to_vec());
        let state = test_app_state(&temp, driver.clone()).await;
        let app = bot_desktop_input_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bots/bot-1/desktop/files/download?sandbox_id=sandbox-abc&path=/tmp/test.txt")
                    .extension(test_user("user-1"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"file contents");
        let calls = driver.recorded();
        assert!(calls
            .iter()
            .any(|c| c == "pull_file:sandbox-abc:/tmp/test.txt"));
    }

    #[tokio::test]
    async fn upload_endpoint_stores_file_bytes() {
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        let state = test_app_state(&temp, driver.clone()).await;
        let app = bot_desktop_input_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bots/bot-1/desktop/files/upload?sandbox_id=sandbox-abc&path=/tmp/upload.txt")
                    .extension(test_user("user-1"))
                    .header("content-type", "application/octet-stream")
                    .body(Body::from(b"uploaded bytes".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let files = driver.files.lock().unwrap();
        assert_eq!(
            files.get("/tmp/upload.txt").unwrap().as_slice(),
            b"uploaded bytes"
        );
        let calls = driver.recorded();
        assert!(calls
            .iter()
            .any(|c| c == "push_file:sandbox-abc:/tmp/upload.txt"));
    }
}

#[cfg(test)]
mod file_path_scope_tests {
    use super::*;

    /// Every caller of the file cores (computer routes, bot desktop routes)
    /// gets the guest-path check, not only the routes that remember it.
    #[tokio::test]
    async fn file_cores_refuse_secret_guest_paths() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(dir.path()).await;
        let down = download_desktop_file_core(&state, "sb", "linux", "incus", None, FilePathQuery { path: "/root/.ssh/id_rsa".into() }).await;
        assert_eq!(down.status(), StatusCode::FORBIDDEN);
        let up = upload_desktop_file_core(
            &state,
            "sb",
            "linux",
            "incus",
            None,
            FilePathQuery { path: "/home/u/.ssh/authorized_keys".into() },
            Bytes::from_static(b"ssh-ed25519 AAAA"),
        )
        .await;
        assert_eq!(up.status(), StatusCode::FORBIDDEN);
    }
}

#[cfg(test)]
mod computer_mouse_tests {
    use super::*;
    #[test]
    fn computer_drag_and_scroll_commands() {
        let drag: MouseInput =
            serde_json::from_value(json!({"action":"drag", "x":10,"y":20,"end_x":100,"end_y":200}))
                .unwrap();
        let cmd = build_mouse_command(&drag, ":99").unwrap();
        assert!(cmd.windows(2).any(|w| w == ["mousedown", "1"]));
        assert_eq!(&cmd[cmd.len() - 2..], ["mouseup", "1"]);
        assert!(cmd.windows(2).any(|w| w == ["100", "200"]));
        let missing: MouseInput = serde_json::from_value(json!({"action":"drag"})).unwrap();
        assert!(build_mouse_command(&missing, ":0").is_err());
        let scroll: MouseInput = serde_json::from_value(json!({"action":"scroll"})).unwrap();
        assert_eq!(
            &build_mouse_command(&scroll, ":0").unwrap()[3..],
            ["click", "--repeat", "3", "5"]
        );
        let up: MouseInput =
            serde_json::from_value(json!({"action":"scroll", "button":"up", "amount":2})).unwrap();
        assert_eq!(
            &build_mouse_command(&up, ":0").unwrap()[3..],
            ["click", "--repeat", "2", "4"]
        );
    }
}
