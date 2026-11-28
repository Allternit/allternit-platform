//! Unified computer control plane.
//!
//! Shared implementation for the `/api/v1/computers/:id/*` control surface and
//! the `computer_*` tool handlers.  The logic is kept in one module so both
//! the REST routes and the tool registry execute guest commands the same way.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::{json, Value};
use tracing::warn;

use crate::bot_desktop_input::{
    build_keyboard_command, build_mouse_command, desktop_display, KeyboardInput, MouseInput,
    ShellInput,
};
use crate::computer_routes::{ComputerKind, ComputerResponse};
use crate::AppState;
use allternit_driver_interface::CommandSpec;

// ---------------------------------------------------------------------------
// Action abstraction shared by REST routes and tool handlers.
// ---------------------------------------------------------------------------

pub(crate) enum ComputerControlAction {
    Screenshot,
    Mouse(MouseInput),
    Keyboard(KeyboardInput),
    Shell(ShellInput),
    FileRead { path: String },
    FileWrite { path: String, content_base64: String },
    /// Enable the authenticated in-VM HTTP proxy on `port` (Phase 3).
    ProxyEnable { port: u16 },
    /// Disable the in-VM HTTP proxy (Phase 3).
    ProxyDisable,
}

/// Canonical action descriptor for grant binding. Must be built
/// deterministically from the request payload only — no ephemeral ids, no
/// per-call randomness — so the same action always hashes identically.
pub(crate) fn control_action_descriptor(action: &ComputerControlAction) -> Value {
    match action {
        ComputerControlAction::Screenshot => json!({ "route": "computer.screenshot" }),
        ComputerControlAction::Mouse(input) => json!({
            "route": "computer.mouse",
            "action": input.action,
            "x": input.x,
            "y": input.y,
            "button": input.button,
        }),
        ComputerControlAction::Keyboard(input) => json!({
            "route": "computer.keyboard",
            "action": input.action,
            "text": input.text,
            "key": input.key,
        }),
        ComputerControlAction::Shell(input) => json!({
            "route": "computer.shell",
            "command": input.command,
        }),
        ComputerControlAction::FileRead { path } => json!({
            "route": "computer.file_read",
            "path": path,
        }),
        ComputerControlAction::FileWrite { path, .. } => json!({
            "route": "computer.file_write",
            "path": path,
        }),
        ComputerControlAction::ProxyEnable { port } => json!({
            "route": "computer.proxy_enable",
            "port": port,
        }),
        ComputerControlAction::ProxyDisable => json!({
            "route": "computer.proxy_disable",
        }),
    }
}

/// Confirmation taxonomy class for a control action (see `aci_safety`).
pub(crate) fn classify_control_action(action: &ComputerControlAction) -> crate::aci_safety::ConfirmationClass {
    use crate::aci_safety::ConfirmationClass;
    match action {
        ComputerControlAction::Screenshot => ConfirmationClass::Reversible,
        ComputerControlAction::Mouse(input) => {
            crate::aci_safety::classify_mouse_action(&input.action)
        }
        ComputerControlAction::Keyboard(input) => crate::aci_safety::classify_keyboard_action(
            &input.action,
            input.text.as_deref(),
            input.key.as_deref(),
        ),
        ComputerControlAction::Shell(input) => {
            crate::aci_safety::classify_shell_command(&input.command)
        }
        ComputerControlAction::FileRead { .. } => ConfirmationClass::Reversible,
        ComputerControlAction::FileWrite { path, .. } => {
            crate::aci_safety::classify_file_write(path)
        }
        // Enabling the proxy exposes an in-guest port to the authenticated
        // caller; disabling removes that exposure. Both mutate state but are
        // directly undoable, so they sit in the risky (approval-gated) tier.
        ComputerControlAction::ProxyEnable { .. } | ComputerControlAction::ProxyDisable => {
            ConfirmationClass::Risky
        }
    }
}

// ---------------------------------------------------------------------------
// Shared implementation.
// ---------------------------------------------------------------------------

/// Canonical policy-seat descriptor for a control action: the tool name and
/// the file path where one applies. `bot_id` is filled in by the seat after a
/// read-only computer lookup (not guest dispatch).
fn control_policy_descriptor(action: &ComputerControlAction) -> crate::policy_config::PolicyDescriptor {
    use ComputerControlAction::*;
    let (tool, path): (&str, Option<String>) = match action {
        Screenshot => ("computer.screenshot", None),
        Mouse(_) => ("computer.mouse", None),
        Keyboard(_) => ("computer.keyboard", None),
        Shell(_) => ("computer.shell", None),
        FileRead { path } => ("computer.file_read", Some(path.clone())),
        FileWrite { path, .. } => ("computer.file_write", Some(path.clone())),
        ProxyEnable { .. } => ("computer.proxy_enable", None),
        ProxyDisable => ("computer.proxy_disable", None),
    };
    crate::policy_config::PolicyDescriptor {
        tool: tool.to_string(),
        file_path: path,
        ..Default::default()
    }
}

/// Execute a control action against a single computer.  This is the shared
/// implementation behind the REST control routes and the `computer_*` tools.
///
/// `approval_id` carries an action-hash grant (see `aci_approvals`) for
/// risky/irreversible actions; without a valid grant those actions are denied
/// with a `confirmation_required` payload before anything touches the guest.
pub(crate) async fn execute_computer_tool(
    state: &AppState,
    user_id: &str,
    computer_id: &str,
    action: ComputerControlAction,
    approval_id: Option<&str>,
) -> Result<Value, (StatusCode, Value)> {
    // Read-only computer lookup so bot_id can ride on the policy descriptor.
    // This is not guest dispatch. Policy still audits before confirmation or
    // any guest exec.
    let computer = match fetch_computer_for_control(state, user_id, computer_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Err((StatusCode::NOT_FOUND, json!({"error": "computer not found"}))),
        Err(resp) => {
            return Err((resp.status(), json!({"error": "failed to load computer"})));
        }
    };

    // Declarative policy gate (env-configured; entirely absent when
    // ALLTERNIT_ACI_POLICY_FILE is unset). Policy verdict first,
    // confirmation enforcement and guest dispatch second — a denial here
    // never reaches the guest. The audit row is fsynced before either
    // outcome (audit-before-act).
    let mut policy_desc = control_policy_descriptor(&action);
    policy_desc.bot_id = computer.bot_id.clone();
    policy_desc.session_id = computer.session_id.clone();
    if let Some(verdict) = crate::policy_config::evaluate_descriptor(&policy_desc) {
        if let Err(e) =
            crate::policy_config::record_decision(&policy_desc, &verdict, Some(user_id), None)
        {
            warn!(user_id, error = %e, "policy audit write failed; refusing to dispatch");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"error": "policy_audit_unavailable"}),
            ));
        }
        if verdict.action == crate::permission_policy::PermissionAction::Deny {
            warn!(
                user_id,
                rule_id = ?verdict.rule_id,
                "computer control action denied by declarative policy"
            );
            return Err((
                StatusCode::FORBIDDEN,
                crate::policy_config::refusal_json(&verdict),
            ));
        }
    }

    let class = classify_control_action(&action);
    if let Err(denial) = crate::aci_safety::enforce_confirmation(
        &state.approval_store,
        user_id,
        "computer.control",
        class,
        &control_action_descriptor(&action),
        approval_id,
    ) {
        return Err((denial.status, denial.body));
    }

    if computer.kind != ComputerKind::CloudDesktop {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            json!({"error": "control actions are only supported for cloud_desktop computers"}),
        ));
    }

    let bot_id = computer.bot_id.as_ref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "cloud_desktop computer has no bot_id"}),
        )
    })?;

    if !crate::bot_desktop_routes::verify_bot_ownership(state, user_id, bot_id).await {
        return Err((StatusCode::FORBIDDEN, json!({"error": "bot not found or access denied"})));
    }

    let record = match crate::bot_desktop_routes::read_bot_sandbox(&state.db, bot_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Err((StatusCode::NOT_FOUND, json!({"error": "no desktop sandbox found for this bot"})));
        }
        Err(e) => {
            warn!(bot_id, error = %e, "failed to read bot desktop sandbox");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "database error"})));
        }
    };

    let driver = match state.vm_driver.as_ref() {
        Some(d) => d.clone(),
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error": "No VM driver is configured on this host"}),
            ));
        }
    };

    let handle = crate::bot_desktop_routes::build_handle(
        &record.sandbox_id,
        Some(&record.os),
        Some(&record.provider),
    );

    match action {
        ComputerControlAction::Screenshot => {
            let png = capture_screenshot(&*driver, &record, bot_id)
                .await
                .map_err(|(s, m)| (s, json!({"error": m})))?;
            Ok(json!({
                "base64": BASE64_STANDARD.encode(&png),
                "content_type": "image/png",
                "size": png.len(),
            }))
        }
        ComputerControlAction::Mouse(input) => {
            let command = if record.os == "windows" {
                build_windows_mouse_command(&input).map_err(|m| (StatusCode::BAD_REQUEST, json!({"error": m})))?
            } else {
                build_mouse_command(&input, desktop_display(&record.provider))
                    .map_err(|m| (StatusCode::BAD_REQUEST, json!({"error": m})))?
            };
            run_guest_command(&*driver, &handle, command, "mouse", bot_id, &record.sandbox_id)
                .await
                .map(|_| json!({ "success": true }))
                .map_err(|(s, m)| (s, json!({ "error": m })))
        }
        ComputerControlAction::Keyboard(input) => {
            let command = if record.os == "windows" {
                build_windows_keyboard_command(&input).map_err(|m| (StatusCode::BAD_REQUEST, json!({"error": m})))?
            } else {
                build_keyboard_command(&input, desktop_display(&record.provider))
                    .map_err(|m| (StatusCode::BAD_REQUEST, json!({"error": m})))?
            };
            run_guest_command(&*driver, &handle, command, "keyboard", bot_id, &record.sandbox_id)
                .await
                .map(|_| json!({ "success": true }))
                .map_err(|(s, m)| (s, json!({ "error": m })))
        }
        ComputerControlAction::Shell(input) => {
            let cmd_spec = if record.os == "windows" {
                crate::bot_desktop_windows::shell_command(&input.command.join(" "))
            } else {
                let mut env_vars = std::collections::HashMap::new();
                env_vars.insert(
                    "DISPLAY".to_string(),
                    desktop_display(&record.provider).to_string(),
                );
                env_vars.extend(input.env);
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
                    Ok(json!({
                        "exit_code": result.exit_code,
                        "stdout": stdout,
                        "stderr": stderr,
                        "duration_ms": result.duration_ms,
                    }))
                }
                Err(e) => {
                    warn!(bot_id, sandbox_id = %record.sandbox_id, error = %e, "failed to run computer shell");
                    Err((StatusCode::SERVICE_UNAVAILABLE, json!({"error": format!("failed to run shell: {}", e)})))
                }
            }
        }
        ComputerControlAction::FileRead { path } => {
            match driver.pull_file(&handle, &path).await {
                Ok(bytes) => Ok(json!({
                    "path": path,
                    "content": BASE64_STANDARD.encode(&bytes),
                    "size": bytes.len(),
                })),
                Err(e) => {
                    warn!(bot_id, sandbox_id = %record.sandbox_id, path = %path, error = %e, "failed to download computer file");
                    Err((StatusCode::SERVICE_UNAVAILABLE, json!({"error": format!("failed to download file: {}", e)})))
                }
            }
        }
        ComputerControlAction::FileWrite { path, content_base64 } => {
            let bytes = BASE64_STANDARD
                .decode(&content_base64)
                .map_err(|e| (StatusCode::BAD_REQUEST, json!({"error": format!("invalid base64 content: {}", e)})))?;
            match driver.push_file(&handle, &path, bytes).await {
                Ok(()) => Ok(json!({ "path": path, "written": true })),
                Err(e) => {
                    warn!(bot_id, sandbox_id = %record.sandbox_id, path = %path, error = %e, "failed to upload computer file");
                    Err((StatusCode::SERVICE_UNAVAILABLE, json!({"error": format!("failed to upload file: {}", e)})))
                }
            }
        }
        // Proxy enable/disable are ACI-gated REST actions managed by the
        // computer proxy routes (computer_ws); they never execute on the
        // guest tool surface.
        ComputerControlAction::ProxyEnable { .. } | ComputerControlAction::ProxyDisable => Err((
            StatusCode::BAD_REQUEST,
            json!({"error": "proxy actions are managed via the computer proxy routes"}),
        )),
    }
}

async fn fetch_computer_for_control(
    state: &AppState,
    user_id: &str,
    id: &str,
) -> Result<Option<ComputerResponse>, axum::response::Response> {
    let db = state.db.clone();
    let user_id = user_id.to_string();
    let id_owned = id.to_string();
    let id_for_error = id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.kind, c.provider, c.status, c.owner_type, c.owner_id, \
             c.bot_id, c.session_id, c.name, c.os, c.cpu_cores, c.memory_mb, c.disk_mb, \
             c.region, c.host, c.native_id, c.template_id, c.billing_source, \
             c.created_at, c.updated_at, c.idle_timeout_secs, c.last_activity_at, c.group_id, c.role \
             FROM computers c \
             LEFT JOIN agents a ON a.id = c.bot_id \
             WHERE c.id = ?1 AND (c.owner_id = ?2 OR (c.kind = 'cloud_desktop' AND a.user_id = ?2)) AND c.status != 'deleted'"
        )?;
        let row = stmt.query_row(rusqlite::params![id_owned, user_id], |row| {
            Ok(ComputerResponse {
                id: row.get(0)?,
                kind: match row.get::<_, String>(1)?.as_str() {
                    "cloud_desktop" => ComputerKind::CloudDesktop,
                    "managed" => ComputerKind::Managed,
                    "byo_vps" => ComputerKind::ByoVps,
                    "byoc" => ComputerKind::Byoc,
                    _ => ComputerKind::Local,
                },
                provider: row.get(2)?,
                status: status_from_str(&row.get::<_, String>(3)?),
                owner_type: row.get(4)?,
                owner_id: row.get(5)?,
                bot_id: row.get(6)?,
                session_id: row.get(7)?,
                name: row.get(8)?,
                os: row.get(9)?,
                cpu_cores: row.get(10)?,
                memory_mb: row.get(11)?,
                disk_mb: row.get(12)?,
                region: row.get(13)?,
                host: row.get(14)?,
                native_id: row.get(15)?,
                template_id: row.get(16)?,
                billing_source: row.get(17)?,
                created_at: row.get(18)?,
                updated_at: row.get(19)?,
                idle_timeout_secs: row.get(20)?,
                last_activity_at: row.get(21)?,
                group_id: row.get(22)?,
                role: row
                    .get::<_, Option<String>>(23)?
                    .unwrap_or_else(|| "user".to_string()),
            })
        });
        match row {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await;

    match result {
        Ok(Ok(computer)) => Ok(computer),
        Ok(Err(e)) => {
            warn!(computer_id = %id_for_error, error = %e, "failed to fetch computer for control");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("database error: {}", e),
            ))
        }
        Err(e) => {
            warn!(computer_id = %id_for_error, error = %e, "task panicked fetching computer for control");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
    }
}

async fn capture_screenshot(
    driver: &dyn allternit_driver_interface::ExecutionDriver,
    record: &crate::bot_desktop_routes::BotDesktopSandboxRecord,
    bot_id: &str,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let handle = crate::bot_desktop_routes::build_handle(
        &record.sandbox_id,
        Some(&record.os),
        Some(&record.provider),
    );
    let capture_cmd = if record.os == "windows" {
        crate::bot_desktop_windows::screenshot_command()
    } else {
        let display = desktop_display(&record.provider);
        let mut env_vars = std::collections::HashMap::new();
        env_vars.insert("DISPLAY".to_string(), display.to_string());
        CommandSpec {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!(
                    "DISPLAY={} scrot -z -o /tmp/allternit-screen.png && base64 -w0 /tmp/allternit-screen.png",
                    display
                ),
            ],
            env_vars,
            working_dir: None,
            stdin_data: None,
            capture_stdout: true,
            capture_stderr: true,
        }
    };

    let exec_result = match driver.exec(&handle, capture_cmd).await {
        Ok(r) => r,
        Err(e) => {
            warn!(bot_id, sandbox_id = %record.sandbox_id, error = %e, "failed to capture computer screenshot");
            return Err((StatusCode::SERVICE_UNAVAILABLE, format!("failed to capture screenshot: {}", e)));
        }
    };

    let stdout = exec_result.stdout.as_deref().unwrap_or(&[]);
    let stdout_str = String::from_utf8_lossy(stdout);
    let stdout_trimmed = stdout_str.trim();
    if stdout_trimmed.is_empty() {
        let stderr = String::from_utf8_lossy(exec_result.stderr.as_deref().unwrap_or(&[]));
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "screenshot command produced no output: exit_code={:?} stderr={}",
                exec_result.exit_code,
                stderr.trim()
            ),
        ));
    }

    match BASE64_STANDARD.decode(stdout_trimmed) {
        Ok(bytes) => Ok(bytes),
        Err(e) => {
            warn!(bot_id, sandbox_id = %record.sandbox_id, error = %e, "screenshot output was not valid base64");
            Err((StatusCode::SERVICE_UNAVAILABLE, format!("invalid screenshot output: {}", e)))
        }
    }
}

async fn run_guest_command(
    driver: &dyn allternit_driver_interface::ExecutionDriver,
    handle: &allternit_driver_interface::ExecutionHandle,
    command: Vec<String>,
    command_kind: &str,
    bot_id: &str,
    sandbox_id: &str,
) -> Result<(), (StatusCode, String)> {
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

    match driver.exec(handle, cmd_spec).await {
        Ok(result) => {
            if result.exit_code != 0 {
                let stderr = String::from_utf8_lossy(result.stderr.as_deref().unwrap_or(&[]));
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "{} command failed: exit_code={} stderr={}",
                        command_kind,
                        result.exit_code,
                        stderr.trim()
                    ),
                ))
            } else {
                Ok(())
            }
        }
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, error = %e, "failed to send computer {} input", command_kind);
            Err((StatusCode::SERVICE_UNAVAILABLE, format!("failed to send {} input: {}", command_kind, e)))
        }
    }
}

fn status_from_str(s: &str) -> crate::computer_routes::ComputerStatus {
    match s {
        "running" => crate::computer_routes::ComputerStatus::Running,
        "stopped" => crate::computer_routes::ComputerStatus::Stopped,
        "creating" => crate::computer_routes::ComputerStatus::Creating,
        "error" => crate::computer_routes::ComputerStatus::Error,
        _ => crate::computer_routes::ComputerStatus::Error,
    }
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn build_windows_mouse_command(input: &MouseInput) -> Result<Vec<String>, String> {
    let action = input.action.to_lowercase();
    let x = input.x.unwrap_or(0);
    let y = input.y.unwrap_or(0);
    let mut cmd = crate::bot_desktop_windows::mouse_move_command(x, y).command;
    match action.as_str() {
        "move" => Ok(cmd),
        "click" => {
            cmd.extend(crate::bot_desktop_windows::mouse_click_command().command);
            Ok(cmd)
        }
        other => Err(format!("unsupported windows mouse action: {}", other)),
    }
}

fn build_windows_keyboard_command(input: &KeyboardInput) -> Result<Vec<String>, String> {
    let action = input.action.to_lowercase();
    match action.as_str() {
        "type" => {
            let text = input
                .text
                .as_deref()
                .ok_or_else(|| "text is required for action=type".to_string())?;
            Ok(crate::bot_desktop_windows::keyboard_type_command(text).command)
        }
        other => Err(format!("unsupported windows keyboard action: {}", other)),
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_driver_interface::{
        Artifact, CommandSpec, DriverCapabilities, DriverFeatures, DriverHealth, DriverType,
        EnvSpecType, ExecutionHandle, ExecutionId, IsolationLevel, LogEntry, Receipt,
        ResourceConsumption, ResourceSpec, SpawnSpec, TenantId,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct MockExecutionDriver {
        calls: Arc<Mutex<Vec<String>>>,
        exec_result: Arc<Mutex<Option<std::result::Result<allternit_driver_interface::ExecResult, allternit_driver_interface::DriverError>>>>,
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
            result: std::result::Result<allternit_driver_interface::ExecResult, allternit_driver_interface::DriverError>,
        ) {
            *self.exec_result.lock().unwrap() = Some(result);
        }

        fn seed_file(&self, path: &str, content: Vec<u8>) {
            self.files.lock().unwrap().insert(path.to_string(), content);
        }
    }

    #[async_trait]
    impl allternit_driver_interface::ExecutionDriver for MockExecutionDriver {
        fn capabilities(&self) -> DriverCapabilities {
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

        async fn spawn(&self, _spec: SpawnSpec) -> std::result::Result<ExecutionHandle, allternit_driver_interface::DriverError> {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "spawn".to_string(),
            })
        }

        async fn pause_vm(&self, _handle: &ExecutionHandle) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn resume_vm(&self, _handle: &ExecutionHandle) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn exec(
            &self,
            handle: &ExecutionHandle,
            _cmd: CommandSpec,
        ) -> std::result::Result<allternit_driver_interface::ExecResult, allternit_driver_interface::DriverError> {
            self.calls.lock().unwrap().push(format!(
                "exec:{}",
                handle.driver_info.get("native_id").unwrap_or(&"?".to_string())
            ));
            match self.exec_result.lock().unwrap().take() {
                Some(result) => result,
                None => Err(allternit_driver_interface::DriverError::NotSupported {
                    feature: "exec".to_string(),
                }),
            }
        }

        async fn stream_logs(&self, _handle: &ExecutionHandle) -> std::result::Result<Vec<LogEntry>, allternit_driver_interface::DriverError> {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "stream_logs".to_string(),
            })
        }

        async fn get_artifacts(&self, _handle: &ExecutionHandle) -> std::result::Result<Vec<Artifact>, allternit_driver_interface::DriverError> {
            Ok(vec![])
        }

        async fn destroy(&self, _handle: &ExecutionHandle) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn get_consumption(&self, _handle: &ExecutionHandle) -> std::result::Result<ResourceConsumption, allternit_driver_interface::DriverError> {
            Ok(ResourceConsumption::default())
        }

        async fn get_receipt(&self, _handle: &ExecutionHandle) -> std::result::Result<Option<Receipt>, allternit_driver_interface::DriverError> {
            Ok(None)
        }

        async fn pull_file(&self, handle: &ExecutionHandle, path: &str) -> std::result::Result<Vec<u8>, allternit_driver_interface::DriverError> {
            self.calls.lock().unwrap().push(format!(
                "pull_file:{}:{}",
                handle.driver_info.get("native_id").unwrap_or(&"?".to_string()),
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
            handle: &ExecutionHandle,
            path: &str,
            content: Vec<u8>,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            self.calls.lock().unwrap().push(format!(
                "push_file:{}:{}",
                handle.driver_info.get("native_id").unwrap_or(&"?".to_string()),
                path
            ));
            self.files.lock().unwrap().insert(path.to_string(), content);
            Ok(())
        }

        async fn health_check(&self) -> std::result::Result<DriverHealth, allternit_driver_interface::DriverError> {
            Ok(DriverHealth {
                healthy: true,
                message: Some("mock".to_string()),
                active_executions: 0,
                available_capacity: self.capabilities().max_resources,
                capabilities: vec![],
            })
        }
    }

    /// Serializes against every other test that touches a policy seat or the
    /// computer-use dir env var (see `policy_config::POLICY_TEST_LOCK`).
    fn policy_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::policy_config::POLICY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    async fn test_state(temp: &Path, driver: Arc<MockExecutionDriver>) -> Arc<AppState> {
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
        conn.execute(
            "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, bot_id, name, os, host, native_id, billing_source)
             VALUES (?1, 'cloud_desktop', 'incus', 'running', 'bot', 'bot-1', 'bot-1', 'Bot desktop', 'linux', 'mail', 'sandbox-abc', 'credits')",
            rusqlite::params!["computer-1"],
        )
        .unwrap();
        drop(conn);
        state
    }

    /// Mint an approved, hash-bound grant for `action`, the same shape the
    /// gateway issues on a `confirmation_required` denial.
    fn approve_grant_for(user_id: &str, action: &ComputerControlAction) -> String {
        let hash = crate::aci_approvals::hash_action_payload(&control_action_descriptor(action));
        let id = crate::aci_approvals::GRANTS.issue(user_id, &hash);
        assert!(crate::aci_approvals::GRANTS.approve(&id));
        id
    }

    #[tokio::test]
    async fn screenshot_tool_returns_base64_png() {
        let _policy_guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
        driver.set_exec_result(Ok(allternit_driver_interface::ExecResult {
            exit_code: 0,
            stdout: Some(png_b64.as_bytes().to_vec()),
            stderr: None,
            duration_ms: 12,
            resource_usage: ResourceConsumption::default(),
        }));
        let state = test_state(&temp, driver.clone()).await;

        let result = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::Screenshot,
            None,
        )
        .await
        .unwrap();

        assert!(result.get("base64").unwrap().as_str().unwrap().len() > 0);
        assert_eq!(result["content_type"], "image/png");
        assert!(driver.recorded().iter().any(|c| c.starts_with("exec:sandbox-abc")));
    }

    #[tokio::test]
    async fn shell_tool_returns_command_output() {
        let _policy_guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(allternit_driver_interface::ExecResult {
            exit_code: 0,
            stdout: Some(b"hello stdout".to_vec()),
            stderr: Some(b"hello stderr".to_vec()),
            duration_ms: 42,
            resource_usage: ResourceConsumption::default(),
        }));
        let state = test_state(&temp, driver.clone()).await;

        // Mutating shell commands sit in the risky tier: they need a grant.
        let action = ComputerControlAction::Shell(ShellInput {
            command: vec!["rm".to_string(), "/tmp/scratch-file".to_string()],
            env: HashMap::new(),
            timeout: None,
        });
        let grant = approve_grant_for("user-1", &action);
        let result = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            action,
            Some(&grant),
        )
        .await
        .unwrap();

        assert_eq!(result["stdout"], "hello stdout");
        assert_eq!(result["stderr"], "hello stderr");
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn file_read_tool_returns_base64_content() {
        let _policy_guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.seed_file("/tmp/test.txt", b"file contents".to_vec());
        let state = test_state(&temp, driver.clone()).await;

        let result = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::FileRead {
                path: "/tmp/test.txt".to_string(),
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(result["size"], 13);
        let decoded = BASE64_STANDARD.decode(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, b"file contents");
    }

    #[tokio::test]
    async fn file_write_tool_stores_content() {
        let _policy_guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        let state = test_state(&temp, driver.clone()).await;

        // File writes are risky-tier: mint a grant bound to this exact action.
        let action = ComputerControlAction::FileWrite {
            path: "/tmp/upload.txt".to_string(),
            content_base64: BASE64_STANDARD.encode(b"uploaded bytes"),
        };
        let grant = approve_grant_for("user-1", &action);
        let result = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            action,
            Some(&grant),
        )
        .await
        .unwrap();

        assert_eq!(result["written"], true);
        let files = driver.files.lock().unwrap();
        assert_eq!(
            files.get("/tmp/upload.txt").unwrap().as_slice(),
            b"uploaded bytes"
        );
    }

    #[tokio::test]
    async fn risky_action_without_grant_is_denied_before_touching_guest() {
        let _policy_guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(allternit_driver_interface::ExecResult {
            exit_code: 0,
            stdout: Some(b"should never run".to_vec()),
            stderr: None,
            duration_ms: 1,
            resource_usage: ResourceConsumption::default(),
        }));
        let state = test_state(&temp, driver.clone()).await;

        let (status, body) = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::Shell(ShellInput {
                command: vec!["rm".to_string(), "/tmp/scratch".to_string()],
                env: HashMap::new(),
                timeout: None,
            }),
            None,
        )
        .await
        .expect_err("risky shell without a grant must be denied");

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "confirmation_required");
        assert!(body["approval_id"].as_str().is_some());
        assert!(body["action_hash"].as_str().is_some());
        // The guest was never touched.
        assert!(driver.recorded().is_empty());
    }

    #[tokio::test]
    async fn grant_bound_to_other_action_is_denied() {
        let _policy_guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(allternit_driver_interface::ExecResult {
            exit_code: 0,
            stdout: Some(b"should never run".to_vec()),
            stderr: None,
            duration_ms: 1,
            resource_usage: ResourceConsumption::default(),
        }));
        let state = test_state(&temp, driver.clone()).await;

        // Grant bound to `rm a`, action tries `rm b` — hash mismatch.
        let granted_action = ComputerControlAction::Shell(ShellInput {
            command: vec!["rm".to_string(), "a".to_string()],
            env: HashMap::new(),
            timeout: None,
        });
        let grant = approve_grant_for("user-1", &granted_action);

        let (status, body) = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::Shell(ShellInput {
                command: vec!["rm".to_string(), "b".to_string()],
                env: HashMap::new(),
                timeout: None,
            }),
            Some(&grant),
        )
        .await
        .expect_err("hash mismatch must be denied");

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "approval_denied");
        assert!(body["reason"]
            .as_str()
            .unwrap()
            .contains("does not match"));
        assert!(driver.recorded().is_empty());
    }

    #[tokio::test]
    async fn reversible_shell_command_needs_no_grant() {
        let _policy_guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(allternit_driver_interface::ExecResult {
            exit_code: 0,
            stdout: Some(b"listing".to_vec()),
            stderr: None,
            duration_ms: 1,
            resource_usage: ResourceConsumption::default(),
        }));
        let state = test_state(&temp, driver.clone()).await;

        let result = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::Shell(ShellInput {
                command: vec!["ls".to_string(), "-la".to_string()],
                env: HashMap::new(),
                timeout: None,
            }),
            None,
        )
        .await
        .unwrap();

        assert_eq!(result["stdout"], "listing");
    }

    // ── Declarative policy seat tests ────────────────────────────────────

    struct ControlPolicyGuard(Option<crate::permission_policy::PermissionPolicy>);

    impl ControlPolicyGuard {
        fn install(rules: Vec<crate::permission_policy::PermissionRule>) -> Self {
            let prior = crate::policy_config::active_policy();
            crate::policy_config::install(Some(crate::permission_policy::PermissionPolicy {
                name: "control-seat-tests".to_string(),
                rules,
            }));
            Self(prior)
        }
    }

    impl Drop for ControlPolicyGuard {
        fn drop(&mut self) {
            crate::policy_config::install(self.0.take());
        }
    }

    fn deny_file_read() -> crate::permission_policy::PermissionRule {
        crate::permission_policy::PermissionRule {
            id: Some("no-file-read".to_string()),
            tool: Some("computer.file_read".to_string()),
            action: crate::permission_policy::PermissionAction::Deny,
            ..Default::default()
        }
    }

    fn allow_all_rule() -> crate::permission_policy::PermissionRule {
        crate::permission_policy::PermissionRule {
            id: Some("allow-all".to_string()),
            tool: Some("*".to_string()),
            action: crate::permission_policy::PermissionAction::Allow,
            ..Default::default()
        }
    }

    fn audit_lines_for_test() -> Vec<serde_json::Value> {
        let Ok(text) = std::fs::read_to_string(crate::policy_audit::policy_audit_path()) else {
            return Vec::new();
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    #[tokio::test]
    async fn policy_deny_blocks_file_read_before_guest_and_audits() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = ControlPolicyGuard::install(vec![deny_file_read(), allow_all_rule()]);

        let driver = Arc::new(MockExecutionDriver::new());
        driver.seed_file("/tmp/test.txt", b"file contents".to_vec());
        let state = test_state(&temp, driver.clone()).await;

        let (status, body) = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::FileRead {
                path: "/tmp/test.txt".to_string(),
            },
            None,
        )
        .await
        .expect_err("policy deny must refuse the action");

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "policy_denied");
        assert_eq!(body["rule_id"], "no-file-read");
        assert!(driver.recorded().is_empty(), "guest must not be touched");

        let row = audit_lines_for_test()
            .into_iter()
            .find(|row| row["tool"] == "computer.file_read")
            .expect("denial audit row");
        assert_eq!(row["decision"], "denied");
        assert_eq!(row["path"], "/tmp/test.txt");
        assert_eq!(row["actor"], "user-1");
    }

    #[tokio::test]
    async fn policy_allow_records_row_and_action_proceeds() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = ControlPolicyGuard::install(vec![allow_all_rule()]);

        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(allternit_driver_interface::ExecResult {
            exit_code: 0,
            stdout: Some(b"listing".to_vec()),
            stderr: None,
            duration_ms: 1,
            resource_usage: ResourceConsumption::default(),
        }));
        let state = test_state(&temp, driver.clone()).await;

        let result = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::Shell(ShellInput {
                command: vec!["ls".to_string()],
                env: HashMap::new(),
                timeout: None,
            }),
            None,
        )
        .await
        .unwrap();

        assert_eq!(result["stdout"], "listing");
        let row = audit_lines_for_test()
            .into_iter()
            .find(|row| row["tool"] == "computer.shell")
            .expect("allowed audit row");
        assert_eq!(row["decision"], "allowed");
        assert_eq!(row["rule_id"], "allow-all");
        assert_eq!(row["bot_id"], "bot-1");
    }

    #[tokio::test]
    async fn policy_bot_scoped_rule_does_not_match_other_bot() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        // Rule is scoped to a different bot than computer-1's bot-1.
        let _policy = ControlPolicyGuard::install(vec![crate::permission_policy::PermissionRule {
            id: Some("other-bot-only".to_string()),
            tool: Some("computer.screenshot".to_string()),
            bot_id: Some("bot-someone-else".to_string()),
            action: crate::permission_policy::PermissionAction::Allow,
            ..Default::default()
        }]);

        let driver = Arc::new(MockExecutionDriver::new());
        let state = test_state(&temp, driver.clone()).await;

        let (status, body) = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::Screenshot,
            None,
        )
        .await
        .expect_err("no matching allow → fail-closed deny");

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "policy_denied");
        assert!(driver.recorded().is_empty());
    }

    #[tokio::test]
    async fn policy_bot_scoped_allow_matches_computer_bot_id() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = ControlPolicyGuard::install(vec![crate::permission_policy::PermissionRule {
            id: Some("this-bot-shell".to_string()),
            tool: Some("computer.shell".to_string()),
            bot_id: Some("bot-1".to_string()),
            action: crate::permission_policy::PermissionAction::Allow,
            ..Default::default()
        }]);

        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(allternit_driver_interface::ExecResult {
            exit_code: 0,
            stdout: Some(b"ok".to_vec()),
            stderr: None,
            duration_ms: 1,
            resource_usage: ResourceConsumption::default(),
        }));
        let state = test_state(&temp, driver.clone()).await;

        let result = execute_computer_tool(
            &state,
            "user-1",
            "computer-1",
            ComputerControlAction::Shell(ShellInput {
                command: vec!["ls".to_string()],
                env: HashMap::new(),
                timeout: None,
            }),
            None,
        )
        .await
        .unwrap();

        assert_eq!(result["stdout"], "ok");
        let row = audit_lines_for_test()
            .into_iter()
            .find(|row| row["tool"] == "computer.shell")
            .expect("allowed audit row");
        assert_eq!(row["decision"], "allowed");
        assert_eq!(row["bot_id"], "bot-1");
        assert_eq!(row["rule_id"], "this-bot-shell");
    }
}
