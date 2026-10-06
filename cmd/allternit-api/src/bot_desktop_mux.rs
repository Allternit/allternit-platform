//! Run a command in a bot desktop's terminal.
//!
//! `POST /bots/:bot_id/desktop/mux/run` (path kept for existing callers)
//! opens a terminal on the guest's Allternit Factory pane engine
//! (`/opt/allternit-factory/allternit-factory pane tty`), types the command,
//! and returns the visible screen. The terminal stays open in the guest's
//! agent session, so the same pane can be watched or continued later.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::bot_desktop_routes::{build_handle, verify_bot_ownership, DesktopQuery};
use crate::AppState;
use allternit_driver_interface::CommandSpec;

pub fn bot_desktop_mux_router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/bots/:bot_id/desktop/mux/run",
        axum::routing::post(run_desktop_mux),
    )
}

#[derive(Debug, Deserialize)]
pub struct MuxRunInput {
    /// Command and arguments to type into the terminal.
    pub command: Vec<String>,
    /// Optional label for the terminal (its workspace in the agent wall).
    pub session_label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MuxRunResponse {
    pub session_id: String,
    pub pane_id: String,
    pub output: String,
}

pub(crate) async fn run_desktop_mux(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Query(query): Query<DesktopQuery>,
    Json(input): Json<MuxRunInput>,
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

    let sandbox_id = match crate::bot_desktop_routes::resolve_sandbox_id(&state, &bot_id, &query).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

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

    let factory_bin = "/opt/allternit-factory/allternit-factory";
    let session_label = input.session_label.unwrap_or_else(|| "desktop".to_string());
    // Terminal ids are [A-Za-z0-9_-]{1,64}: the label, cleaned, plus a
    // unique suffix so repeated runs never collide.
    let label_part: String = session_label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .take(40)
        .collect();
    let terminal_id = format!("{label_part}-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    // Reconstruct the argv as a single shell-escaped command string.
    let command_shell: String = input
        .command
        .iter()
        .map(|arg| shell_escape(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let command_b64 = BASE64_STANDARD.encode(command_shell);

    // One-shot script inside the guest:
    // 1. Open a terminal on the pane engine (`tty create` starts the engine
    //    when it is down).
    // 2. Decode and type the command.
    // 3. Wait briefly for output, then read the visible screen.
    // 4. Emit JSON with session_id (the terminal id), pane_id, and output.
    let script = format!(
        r#"set -e
export HOME=/root
export HERDR_SESSION=ao
FACTORY={factory_bin}
TID={terminal_id}
CREATED=$($FACTORY pane tty create "$TID" --label {session_label} --cols 120 --rows 40)
PANE=$(printf '%s' "$CREATED" | python3 -c 'import sys,json; print(json.load(sys.stdin)["terminal"]["pane_id"] or "")')
CMD=$(printf '%s' '{command_b64}' | base64 -d)
$FACTORY pane tty write "$TID" "$CMD" --enter >/dev/null
sleep 1
OUTPUT=$($FACTORY pane pane read "$PANE" --source visible)
python3 -c 'import json,sys; print(json.dumps({{"session_id":sys.argv[1],"pane_id":sys.argv[2],"output":sys.argv[3]}}))' "$TID" "$PANE" "$OUTPUT"
"#,
        factory_bin = factory_bin,
        terminal_id = shell_escape(&terminal_id),
        session_label = shell_escape(&session_label),
        command_b64 = command_b64,
    );

    let handle = build_handle(&sandbox_id, None, None);
    let mut env_vars = std::collections::HashMap::new();
    env_vars.insert("DISPLAY".to_string(), ":0".to_string());
    let cmd_spec = CommandSpec {
        command: vec!["bash".to_string(), "-c".to_string(), script],
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
                        "error": "desktop terminal run failed",
                        "exit_code": result.exit_code,
                        "stderr": stderr.trim(),
                    })),
                )
                    .into_response();
            }
            let stdout = String::from_utf8_lossy(result.stdout.as_deref().unwrap_or(&[]));
            let stdout = stdout.trim();
            match serde_json::from_str::<MuxRunResponse>(stdout) {
                Ok(resp) => (
                    StatusCode::OK,
                    Json(json!({
                        "session_id": resp.session_id,
                        "pane_id": resp.pane_id,
                        "output": resp.output,
                    })),
                )
                    .into_response(),
                Err(e) => {
                    warn!(bot_id, sandbox_id = %sandbox_id, error = %e, stdout = %stdout, "Desktop terminal run produced invalid JSON");
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({
                            "error": format!("invalid terminal output: {}", e),
                            "raw_stdout": stdout,
                        })),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, error = %e, "Failed to run desktop terminal");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to run the desktop terminal: {}", e)})),
            )
                .into_response()
        }
    }
}

fn shell_escape(s: &str) -> String {
    // Replace any single quotes with '\'' so the value can be safely placed
    // inside single quotes in a shell script.
    format!("'{}'", s.replace('\'', "'\\''"))
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
        exec_result: Arc<Mutex<Option<std::result::Result<ExecResult, allternit_driver_interface::DriverError>>>>,
    }

    impl MockExecutionDriver {
        fn new() -> Self {
            Self {
                exec_result: Arc::new(Mutex::new(None)),
            }
        }

        fn set_exec_result(
            &self,
            result: std::result::Result<ExecResult, allternit_driver_interface::DriverError>,
        ) {
            *self.exec_result.lock().unwrap() = Some(result);
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
        ) -> std::result::Result<allternit_driver_interface::ExecutionHandle, allternit_driver_interface::DriverError>
        {
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
            _handle: &allternit_driver_interface::ExecutionHandle,
            _cmd: CommandSpec,
        ) -> std::result::Result<ExecResult, allternit_driver_interface::DriverError> {
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
        ) -> std::result::Result<Vec<allternit_driver_interface::LogEntry>, allternit_driver_interface::DriverError>
        {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "stream_logs".to_string(),
            })
        }

        async fn get_artifacts(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<Vec<allternit_driver_interface::Artifact>, allternit_driver_interface::DriverError>
        {
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
        ) -> std::result::Result<allternit_driver_interface::ResourceConsumption, allternit_driver_interface::DriverError>
        {
            Ok(allternit_driver_interface::ResourceConsumption::default())
        }

        async fn get_receipt(
            &self,
            _handle: &allternit_driver_interface::ExecutionHandle,
        ) -> std::result::Result<Option<allternit_driver_interface::Receipt>, allternit_driver_interface::DriverError>
        {
            Ok(None)
        }

        async fn health_check(
            &self,
        ) -> std::result::Result<allternit_driver_interface::DriverHealth, allternit_driver_interface::DriverError>
        {
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

    #[tokio::test]
    async fn mux_run_endpoint_returns_pane_output() {
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(ExecResult {
            exit_code: 0,
            stdout: Some(b"{\"session_id\":\"1\",\"pane_id\":\"1-1\",\"output\":\"hello mux\"}".to_vec()),
            stderr: None,
            duration_ms: 100,
            resource_usage: allternit_driver_interface::ResourceConsumption::default(),
        }));
        let state = test_app_state(&temp, driver.clone()).await;
        let app = bot_desktop_mux_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bots/bot-1/desktop/mux/run?sandbox_id=sandbox-abc")
                    .extension(test_user("user-1"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"command":["echo","hello mux"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["session_id"], "1");
        assert_eq!(body["pane_id"], "1-1");
        assert_eq!(body["output"], "hello mux");
    }

    #[tokio::test]
    async fn mux_run_endpoint_reports_guest_failure() {
        let temp = tempfile::tempdir().unwrap().keep();
        let driver = Arc::new(MockExecutionDriver::new());
        driver.set_exec_result(Ok(ExecResult {
            exit_code: 1,
            stdout: None,
            stderr: Some(b"allternit-factory not installed".to_vec()),
            duration_ms: 10,
            resource_usage: allternit_driver_interface::ResourceConsumption::default(),
        }));
        let state = test_app_state(&temp, driver.clone()).await;
        let app = bot_desktop_mux_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bots/bot-1/desktop/mux/run?sandbox_id=sandbox-abc")
                    .extension(test_user("user-1"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"command":["echo","hello"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
