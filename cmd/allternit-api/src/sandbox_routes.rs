//! Sandbox API routes for VM-based code execution
//!
//! Replaces Docker-based sandboxing with VM-based execution

use axum::{
    body::Body,
    extract::{Json, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;
use tracing::{debug, error};

use allternit_driver_interface::{
    CommandSpec, EnvironmentSpec, ExecutionDriver, ExecutionId, PolicySpec, ResourceSpec, SpawnSpec,
    TenantId,
};

use crate::AppState;
use crate::env_allowlist::minimal_child_env;

/// Sandbox execution request
#[derive(Debug, Deserialize)]
pub struct SandboxExecuteRequest {
    /// Code or command to execute
    pub code: String,
    /// Language/runtime
    pub language: String,
    /// Working directory
    #[serde(default)]
    pub workdir: Option<String>,
    /// Environment variables
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Resource limits
    #[serde(default)]
    pub resources: Option<ResourceLimitsRequest>,
    /// Toolchain layers
    #[serde(default)]
    pub toolchains: Vec<String>,
    /// Enable network
    #[serde(default)]
    pub network_enabled: bool,
    /// Uploads to stage into the VM workdir as `in/<name>` before the code
    /// runs. Files the code writes to `out/` come back as `files[]`.
    #[serde(default)]
    pub files: Vec<SandboxFileRef>,
}

/// An upload (`POST /api/v1/uploads`) staged into the sandbox as `in/<name>`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxFileRef {
    pub name: String,
    pub upload_id: String,
}

/// A file the code wrote to `out/`, stored as an upload.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxFileOut {
    pub name: String,
    pub media_type: String,
    pub upload_id: String,
}

fn default_timeout() -> u64 {
    300
}

#[derive(Debug, Deserialize)]
pub struct ResourceLimitsRequest {
    pub cpu_cores: Option<f64>,
    pub memory_mb: Option<u64>,
}

/// Sandbox execution response
#[derive(Debug, Serialize)]
pub struct SandboxExecuteResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub session_id: Option<String>,
    /// Files the code wrote to `out/` (only when the request staged `files`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SandboxFileOut>,
}

/// Sandbox capabilities response
#[derive(Debug, Serialize)]
pub struct SandboxCapabilitiesResponse {
    pub driver_type: String,
    pub isolation_level: String,
    pub supported_languages: Vec<String>,
    pub available_toolchains: Vec<String>,
    pub max_concurrent_sessions: usize,
    pub supports_snapshots: bool,
    pub supports_streaming: bool,
}

/// Create sandbox router
pub fn sandbox_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(sandbox_status))
        .route("/execute", post(execute_handler))
        .route("/execute/stream", post(execute_stream_handler))
        .route("/capabilities", get(capabilities_handler))
        .route("/pool", get(pool_status_handler))
        .route("/health", get(health_handler))
}

async fn sandbox_status() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "sandbox",
    }))
}

/// Execute code in sandbox
async fn execute_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SandboxExecuteRequest>,
) -> Result<Json<SandboxExecuteResponse>, (StatusCode, String)> {
    debug!("Sandbox execute request: {:?}", request);

    // Get the execution driver
    let driver = state.vm_driver.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "VM driver not available".to_string(),
    ))?;

    // Warm pooled path: checkout a health-checked VM, exec, checkin. Pool
    // errors fail closed below — a checkout failure is a 503, never a silent
    // unsandboxed fallback.
    if let Some(pool) = crate::vm_pool::global_pool() {
        let response = execute_via_pool(&pool, &request)
            .await
            .map_err(|e| (e.0, e.1))?;
        return Ok(Json(response));
    }

    let response = execute_with_driver(driver, &request)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(response))
}

/// Execute against a pooled VM. Mirrors `execute_with_driver`'s command
/// construction; the VM survives the request and is returned to the pool.
async fn execute_via_pool(
    pool: &Arc<crate::vm_pool::VmPool>,
    request: &SandboxExecuteRequest,
) -> Result<SandboxExecuteResponse, (StatusCode, String)> {
    use allternit_driver_interface::CommandSpec;

    let (command_spec, marker) = sandbox_command(request)?;

    let checkout = pool
        .checkout()
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.message()))?;

    let result = pool
        .exec_on(&checkout, command_spec)
        .await
        .map_err(|e| format!("Execution failed: {e}"));

    match result {
        Ok(exec_result) => {
            // A completed exec (any exit code) proves the VM is alive; return
            // it to the warm pool.
            pool.checkin(checkout.vm_id, true).await;
            let stdout = exec_result
                .stdout
                .map(|v| String::from_utf8_lossy(&v).into_owned())
                .unwrap_or_default();
            let stderr = exec_result
                .stderr
                .map(|v| String::from_utf8_lossy(&v).into_owned())
                .unwrap_or_default();
            let (stdout, files) = collect_out_files(stdout, marker.as_deref());
            Ok(SandboxExecuteResponse {
                exit_code: exec_result.exit_code,
                stdout,
                stderr,
                duration_ms: exec_result.duration_ms,
                session_id: Some(checkout.vm_id.to_string()),
                files,
            })
        }
        Err(message) => {
            // Driver-level exec failure usually means a dead VM: destroy it.
            pool.checkin(checkout.vm_id, false).await;
            Err((StatusCode::INTERNAL_SERVER_ERROR, message))
        }
    }
}

/// Pool status for operators: warm counts, limits, and state-file location.
async fn pool_status_handler() -> Response {
    match crate::vm_pool::global_pool() {
        Some(pool) => Json(serde_json::to_value(pool.stats()).unwrap_or_default()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "vm_pool_disabled",
                "message": "VM pool is disabled or no VM driver is configured.",
                "enabled": false,
            })),
        )
            .into_response(),
    }
}

async fn execute_with_driver(
    driver: &Arc<dyn ExecutionDriver>,
    request: &SandboxExecuteRequest,
) -> Result<SandboxExecuteResponse, String> {
    let spawn_spec = build_spawn_spec(request).map_err(|e| e.1)?;
    let (command_spec, marker) = sandbox_command(request).map_err(|e| e.1)?;

    let handle = driver.spawn(spawn_spec).await.map_err(|e| format!("Failed to spawn: {e}"))?;

    let result = driver
        .exec(&handle, command_spec)
        .await
        .map_err(|e| format!("Execution failed: {e}"))?;

    let _ = driver.destroy(&handle).await;

    let stdout = result
        .stdout
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default();
    let stderr = result
        .stderr
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default();
    let (stdout, files) = collect_out_files(stdout, marker.as_deref());

    Ok(SandboxExecuteResponse {
        exit_code: result.exit_code,
        stdout,
        stderr,
        duration_ms: result.duration_ms,
        session_id: Some(handle.id.to_string()),
        files,
    })
}

/// Best-effort sandbox execution. Uses the VM driver when available; otherwise
/// falls back to a local subprocess so server tools and tests can run without
/// a full WebVM backend.
pub async fn execute_sandbox_or_subprocess(
    state: &AppState,
    request: &SandboxExecuteRequest,
) -> Result<SandboxExecuteResponse, String> {
    if let Some(driver) = state.vm_driver.as_ref() {
        execute_with_driver(driver, request).await
    } else {
        execute_subprocess_fallback(request).await
    }
}

async fn execute_subprocess_fallback(
    request: &SandboxExecuteRequest,
) -> Result<SandboxExecuteResponse, String> {
    if !request.files.is_empty() {
        return Err("files[] needs the VM sandbox; it is not available on this host".to_string());
    }
    let interpreter = get_interpreter(&request.language);
    let mut cmd = tokio::process::Command::new(&interpreter);
    cmd.arg("-c").arg(&request.code);
    // Strip the full API environment before applying the caller's explicit env.
    cmd.env_clear().envs(minimal_child_env(Some(request.env.clone())));
    if let Some(dir) = &request.workdir {
        cmd.current_dir(dir);
    }
    let timeout = Duration::from_secs(request.timeout_secs.max(1));
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| "Execution timed out".to_string())?
        .map_err(|e| format!("Failed to execute: {e}"))?;

    Ok(SandboxExecuteResponse {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        duration_ms: 0,
        session_id: None,
        files: Vec::new(),
    })
}

/// Execute code with streaming output (SSE)
async fn execute_stream_handler(
    State(_state): State<Arc<AppState>>,
    Json(request): Json<SandboxExecuteRequest>,
) -> Response {
    if !request.files.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "files_unsupported", "message": "files[] is supported on POST /sandbox/execute, not /execute/stream."})),
        )
            .into_response();
    }
    let (cmd, args): (&str, Vec<&str>) = match request.language.as_str() {
        "python" | "python3" => ("python3", vec!["-c", &request.code]),
        "node" | "javascript" => ("node", vec!["-e", &request.code]),
        "bash" | "sh" => ("bash", vec!["-c", &request.code]),
        _ => {
            let body = format!(
                "data: {{\"type\":\"error\",\"message\":\"Unsupported language: {}\"}}\n\n",
                request.language
            );
            return Response::builder()
                .status(200)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(body))
                .unwrap_or_default();
        }
    };

    let timeout = std::time::Duration::from_secs(request.timeout_secs.min(300));
    let _code = request.code.clone();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();

    let stream_body = async_stream::stream! {
        let mut child = match tokio::process::Command::new(cmd)
            .args(&args)
            .env_clear()
            .envs(minimal_child_env(Some(request.env.clone())))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("data: {{\"type\":\"error\",\"message\":\"{}\"}}\n\n", e);
                yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(msg));
                return;
            }
        };

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut stdout_lines = tokio::io::BufReader::new(stdout).lines();
        let mut stderr_lines = tokio::io::BufReader::new(stderr).lines();

        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            tokio::select! {
                line = stdout_lines.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            let escaped = text.replace('"', "\\\"");
                            let msg = format!("data: {{\"type\":\"stdout\",\"line\":\"{escaped}\"}}\n\n");
                            yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(msg));
                        }
                        Ok(None) => break,
                        Err(e) => {
                            error!(error=%e, "stdout read error");
                            break;
                        }
                    }
                }
                line = stderr_lines.next_line() => {
                    match line {
                        Ok(Some(text)) => {
                            let escaped = text.replace('"', "\\\"");
                            let msg = format!("data: {{\"type\":\"stderr\",\"line\":\"{escaped}\"}}\n\n");
                            yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(msg));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!(error=%e, "stderr read error");
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    let _ = child.kill().await;
                    let msg = "data: {\"type\":\"error\",\"message\":\"Execution timed out\"}\n\n".to_string();
                    yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(msg));
                    return;
                }
            }
        }

        let exit_code = child.wait().await.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let done = format!("data: {{\"type\":\"done\",\"exit_code\":{exit_code}}}\n\n");
        yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(done));
    };

    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream_body))
        .unwrap_or_default()
}

/// Get sandbox capabilities
async fn capabilities_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SandboxCapabilitiesResponse>, (StatusCode, String)> {
    let caps = if let Some(driver) = state.vm_driver.as_ref() {
        driver.capabilities()
    } else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "VM driver not available".to_string(),
        ));
    };

    let driver_type = format!("{:?}", caps.driver_type).to_lowercase();
    let isolation_level = format!("{:?}", caps.isolation).to_lowercase();

    Ok(Json(SandboxCapabilitiesResponse {
        driver_type,
        isolation_level,
        supported_languages: vec![
            "python".to_string(),
            "node".to_string(),
            "bash".to_string(),
            "rust".to_string(),
        ],
        available_toolchains: vec![
            "python-3.12".to_string(),
            "node-22".to_string(),
            "rust-stable".to_string(),
        ],
        max_concurrent_sessions: 10, // Default or obtain from elsewhere
        supports_snapshots: caps.features.snapshot,
        supports_streaming: true,
    }))
}

/// Health check endpoint
async fn health_handler(
    State(state): State<Arc<AppState>>,
) -> Result<String, (StatusCode, String)> {
    if state.vm_driver.is_none() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "VM driver not initialized".to_string(),
        ));
    }

    Ok("OK".to_string())
}

/// Build SpawnSpec from request
fn build_spawn_spec(request: &SandboxExecuteRequest) -> Result<SpawnSpec, (StatusCode, String)> {
    let resources = if let Some(req) = &request.resources {
        ResourceSpec {
            cpu_millis: (req.cpu_cores.unwrap_or(1.0) * 1000.0) as u32,
            memory_mib: req.memory_mb.unwrap_or(512) as u32,
            ..Default::default()
        }
    } else {
        ResourceSpec::minimal()
    };

    let tenant = TenantId::new(format!("api-{}", uuid::Uuid::new_v4()))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let env = EnvironmentSpec {
        image: "ubuntu-22.04-minimal".to_string(),
        env_vars: request.env.clone(),
        working_dir: request.workdir.clone(),
        ..Default::default()
    };

    Ok(SpawnSpec {
        tenant,
        project: None,
        workspace: None,
        run_id: Some(ExecutionId::new()),
        env,
        policy: PolicySpec::default_permissive(),
        resources,
        envelope: None,
        prewarm_pool: None,
    })
}

/// Get interpreter for language
/// Total bytes `files[]` may stage into, or bring back out of, one run.
const SANDBOX_FILES_MAX_BYTES: usize = 20 * 1024 * 1024;

/// The exec command for a request. Without `files` it is `<interp> -c code`.
/// With `files` it is `sh -s` fed a script on stdin (argv can't carry 20 MB):
/// the script writes each upload to `in/<name>` and the code to a file, runs
/// it, then prints a random marker line and each regular file in `out/` as
/// `name` + base64 lines. Returns the marker so the caller can split stdout.
fn sandbox_command(
    request: &SandboxExecuteRequest,
) -> Result<(CommandSpec, Option<String>), (StatusCode, String)> {
    if request.files.is_empty() {
        return Ok((
            CommandSpec {
                command: vec![get_interpreter(&request.language), "-c".to_string(), request.code.clone()],
                env_vars: request.env.clone(),
                working_dir: request.workdir.clone(),
                stdin_data: None,
                capture_stdout: true,
                capture_stderr: true,
            },
            None,
        ));
    }
    let staged = request
        .files
        .iter()
        .map(|file| {
            let name = file.name.trim();
            let safe = !name.is_empty()
                && name != "."
                && name != ".."
                && !name.contains(['/', '\\', '\'', '\n', '\r', '\0']);
            if !safe {
                return Err((StatusCode::BAD_REQUEST, format!("invalid file name: {:?}", file.name)));
            }
            let (bytes, _, _) = crate::upload_routes::read_upload(&file.upload_id)
                .ok_or((StatusCode::NOT_FOUND, format!("upload not found: {}", file.upload_id)))?;
            Ok((name.to_string(), bytes))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if staged.iter().map(|(_, b)| b.len()).sum::<usize>() > SANDBOX_FILES_MAX_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "files[] is limited to 20MB in total".to_string()));
    }
    let marker = format!("__ALLTERNIT_OUT_{}__", uuid::Uuid::new_v4().simple());
    let script = staging_script(&staged, &request.code, &get_interpreter(&request.language), &marker);
    Ok((
        CommandSpec {
            command: vec!["sh".to_string(), "-s".to_string()],
            env_vars: request.env.clone(),
            working_dir: request.workdir.clone(),
            stdin_data: Some(script.into_bytes()),
            capture_stdout: true,
            capture_stderr: true,
        },
        Some(marker),
    ))
}

fn staging_script(files: &[(String, Vec<u8>)], code: &str, interpreter: &str, marker: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let wrap = |bytes: &[u8]| {
        let b64 = STANDARD.encode(bytes);
        b64.as_bytes()
            .chunks(76)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut s = String::from("set -e\nmkdir -p in out\n");
    for (name, bytes) in files {
        s.push_str(&format!("base64 -d > 'in/{name}' <<'{marker}'\n{}\n{marker}\n", wrap(bytes)));
    }
    s.push_str(&format!("base64 -d > .allternit_main <<'{marker}'\n{}\n{marker}\n", wrap(code.as_bytes())));
    s.push_str(&format!(
        "set +e\n{interpreter} .allternit_main\ncode=$?\nprintf '\\n%s\\n' '{marker}'\n\
         for f in out/*; do [ -f \"$f\" ] && [ ! -L \"$f\" ] || continue; \
         printf '%s\\n' \"${{f#out/}}\"; base64 < \"$f\" | tr -d '\\n'; printf '\\n'; done\n\
         exit $code\n"
    ));
    s
}

/// Split the marker section off stdout: `(clean stdout, [(name, bytes)])`.
/// Without a marker (no `files` staged) stdout is returned as is.
fn split_out_files(stdout: String, marker: Option<&str>) -> (String, Vec<(String, Vec<u8>)>) {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let Some(marker) = marker else { return (stdout, Vec::new()) };
    let Some(at) = stdout.rfind(&format!("\n{marker}\n")) else { return (stdout, Vec::new()) };
    let (clean, tail) = stdout.split_at(at);
    let mut files = Vec::new();
    let mut total = 0usize;
    let mut lines = tail[marker.len() + 2..].lines();
    while let (Some(name), Some(b64)) = (lines.next(), lines.next()) {
        let Ok(bytes) = STANDARD.decode(b64.trim()) else { continue };
        total += bytes.len();
        if total > SANDBOX_FILES_MAX_BYTES {
            break;
        }
        files.push((name.to_string(), bytes));
    }
    (clean.to_string(), files)
}

/// `split_out_files`, then store each `out/` file as an upload.
fn collect_out_files(stdout: String, marker: Option<&str>) -> (String, Vec<SandboxFileOut>) {
    let (clean, raw) = split_out_files(stdout, marker);
    let mut files = Vec::new();
    for (name, bytes) in raw {
        let media_type = media_type_for(&name);
        let id = uuid::Uuid::new_v4().simple().to_string();
        if let Err(e) = crate::upload_routes::store_upload(&id, &name, media_type, &bytes) {
            error!(error = %e, file = %name, "sandbox: failed to store out/ file");
            continue;
        }
        files.push(SandboxFileOut { name, media_type: media_type.to_string(), upload_id: id });
    }
    (clean, files)
}

/// Media type from a file extension (the common run outputs).
fn media_type_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn get_interpreter(language: &str) -> String {
    match language.to_lowercase().as_str() {
        "python" | "py" => "python3".to_string(),
        "node" | "js" | "javascript" => "node".to_string(),
        "bash" | "sh" => "bash".to_string(),
        "rust" | "rs" => "rustc".to_string(),
        _ => language.to_string(),
    }
}

#[cfg(test)]
mod files_tests {
    use super::*;

    #[test]
    fn request_parses_files_and_defaults_to_none() {
        let with: SandboxExecuteRequest = serde_json::from_value(json!({
            "code": "print(1)", "language": "python",
            "files": [{ "name": "data.csv", "uploadId": "abc123" }]
        }))
        .unwrap();
        assert_eq!(with.files.len(), 1);
        assert_eq!(with.files[0].upload_id, "abc123");
        let without: SandboxExecuteRequest =
            serde_json::from_value(json!({ "code": "print(1)", "language": "python" })).unwrap();
        assert!(without.files.is_empty());
        let (spec, marker) = sandbox_command(&without).unwrap();
        assert_eq!(spec.command, vec!["python3", "-c", "print(1)"]);
        assert!(marker.is_none());
    }

    #[test]
    fn rejects_path_like_names() {
        for bad in ["../x", "a/b", "it's", ""] {
            let req: SandboxExecuteRequest = serde_json::from_value(json!({
                "code": "", "language": "python", "files": [{ "name": bad, "uploadId": "x" }]
            }))
            .unwrap();
            assert_eq!(sandbox_command(&req).unwrap_err().0, StatusCode::BAD_REQUEST, "{bad}");
        }
    }

    #[test]
    fn response_omits_empty_files() {
        let resp = SandboxExecuteResponse {
            exit_code: 0, stdout: String::new(), stderr: String::new(),
            duration_ms: 0, session_id: None, files: Vec::new(),
        };
        assert!(serde_json::to_value(&resp).unwrap().get("files").is_none());
    }

    /// The staging script really stages `in/`, runs the code, and returns
    /// `out/` files after the marker (run with the host's `sh` + python3).
    #[test]
    fn staging_script_round_trip_through_sh() {
        use std::io::Write;
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let code = "import csv\nrows=list(csv.reader(open('in/data.csv')))\nprint('rows', len(rows))\n\
                    open('out/total.txt','w').write(str(sum(int(r[1]) for r in rows[1:])))\n\
                    open('out/empty.bin','wb').close()\n";
        let marker = "__ALLTERNIT_OUT_test__";
        let script = staging_script(
            &[("data.csv".to_string(), b"a,b\nx,2\ny,40\n".to_vec())],
            code,
            "python3",
            marker,
        );
        let mut child = std::process::Command::new("sh")
            .arg("-s")
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        let (clean, mut files) = split_out_files(String::from_utf8(out.stdout).unwrap(), Some(marker));
        assert_eq!(clean.trim(), "rows 3");
        files.sort();
        assert_eq!(files, vec![("empty.bin".to_string(), vec![]), ("total.txt".to_string(), b"42".to_vec())]);
        assert_eq!(media_type_for("chart.PNG"), "image/png");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_driver_interface::{
        DriverCapabilities, DriverError, DriverHealth, DriverType, ExecResult, ExecutionHandle,
        IsolationLevel, ResourceConsumption, SpawnSpec, TenantId,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use uuid::Uuid;

    /// Minimal driver: every exec succeeds, spawns return fresh handles, and
    /// spawn/destroy calls are counted so tests can prove pooling.
    #[derive(Debug, Default)]
    struct SmokeMockDriver {
        spawns: Mutex<u32>,
        destroys: Mutex<u32>,
    }

    #[async_trait::async_trait]
    impl allternit_driver_interface::ExecutionDriver for SmokeMockDriver {
        fn capabilities(&self) -> DriverCapabilities {
            DriverCapabilities {
                resize: false,
                clone: false,
                driver_type: DriverType::Container,
                isolation: IsolationLevel::Standard,
                max_resources: ResourceSpec::minimal(),
                supported_env_specs: vec![],
                features: Default::default(),
            }
        }

        async fn spawn(&self, _spec: SpawnSpec) -> Result<ExecutionHandle, DriverError> {
            *self.spawns.lock().unwrap() += 1;
            Ok(ExecutionHandle {
                id: allternit_driver_interface::ExecutionId(Uuid::new_v4()),
                tenant: TenantId::new("allternit-vm-pool".to_string()).unwrap(),
                driver_info: HashMap::new(),
                env_spec: Default::default(),
            })
        }

        async fn pause_vm(&self, _h: &ExecutionHandle) -> Result<(), DriverError> {
            Ok(())
        }
        async fn resume_vm(&self, _h: &ExecutionHandle) -> Result<(), DriverError> {
            Ok(())
        }

        async fn exec(
            &self,
            _h: &ExecutionHandle,
            _cmd: CommandSpec,
        ) -> Result<ExecResult, DriverError> {
            Ok(ExecResult {
                exit_code: 0,
                stdout: Some(b"pooled-ok".to_vec()),
                stderr: None,
                duration_ms: 1,
                resource_usage: ResourceConsumption::default(),
            })
        }

        async fn stream_logs(
            &self,
            _h: &ExecutionHandle,
        ) -> Result<Vec<allternit_driver_interface::LogEntry>, DriverError> {
            Ok(vec![])
        }
        async fn get_artifacts(
            &self,
            _h: &ExecutionHandle,
        ) -> Result<Vec<allternit_driver_interface::Artifact>, DriverError> {
            Ok(vec![])
        }

        async fn destroy(&self, _h: &ExecutionHandle) -> Result<(), DriverError> {
            *self.destroys.lock().unwrap() += 1;
            Ok(())
        }

        async fn get_consumption(&self, _h: &ExecutionHandle) -> Result<ResourceConsumption, DriverError> {
            Ok(Default::default())
        }
        async fn get_receipt(
            &self,
            _h: &ExecutionHandle,
        ) -> Result<Option<allternit_driver_interface::Receipt>, DriverError> {
            Ok(None)
        }

        async fn health_check(&self) -> Result<DriverHealth, DriverError> {
            Ok(DriverHealth {
                healthy: true,
                message: None,
                active_executions: 0,
                available_capacity: ResourceSpec::minimal(),
                capabilities: vec![],
            })
        }
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::Value::Null)
    }

    /// Live HTTP smoke of the pooled sandbox path: boot the sandbox router
    /// over a mock driver, execute twice (second request must reuse the warm
    /// VM), then "restart" the gateway (re-init the pool from the same state
    /// file) and confirm the warm VM survived and is reused — never
    /// respawned, never leaked.
    #[tokio::test]
    async fn pooled_execute_over_http_and_restart_reuse() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state_path = temp.join("pool-state.json");
        let driver = Arc::new(SmokeMockDriver::default());
        let cfg = crate::vm_pool::PoolConfig {
            min_idle: 0,
            max_total: 4,
            idle_ttl: std::time::Duration::from_secs(600),
        };

        // ── Gateway boot #1 ──
        let pool = crate::vm_pool::init_global_for_test(
            driver.clone(),
            cfg,
            Some(state_path.clone()),
        );
        let state = crate::test_helpers::app_state_with_driver(&temp, Some(driver.clone())).await;
        let app = sandbox_router().with_state(state);

        let execute = |code: &str| {
            let app = app.clone();
            let body = serde_json::json!({
                "code": code,
                "language": "bash",
            });
            async move {
                app.oneshot(
                    Request::post("/execute")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        let resp = execute("echo one").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["exit_code"], 0);
        assert_eq!(json["stdout"], "pooled-ok");
        assert_eq!(driver.spawns.lock().unwrap().clone(), 1);

        // Second request: the first VM was checked in warm and must be
        // reused, not respawned.
        let resp = execute("echo two").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(driver.spawns.lock().unwrap().clone(), 1, "warm VM reused");
        assert_eq!(pool.stats().idle, 1);

        // Pool status route reports the warm VM.
        let state2 = crate::test_helpers::app_state(&temp).await;
        let app2 = sandbox_router().with_state(state2);
        let resp = app2
            .oneshot(Request::get("/pool").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["enabled"], true);
        assert_eq!(json["idle"], 1);
        assert_eq!(json["max_total"], 4);

        // ── Gateway boot #2 (simulated restart, same state file) ──
        drop(pool);
        let pool2 = crate::vm_pool::init_global_for_test(driver.clone(), cfg, Some(state_path));
        // The checked-in VM from boot #1 is restored as warm; the checked-out
        // state from boot #1 was empty, so nothing to reclaim.
        assert_eq!(pool2.stats().idle, 1, "warm VM restored from disk");

        let state3 = crate::test_helpers::app_state_with_driver(&temp, Some(driver.clone())).await;
        let app3 = sandbox_router().with_state(state3);
        let resp = app3
            .oneshot(
                Request::post("/execute")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"code": "echo three", "language": "bash"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            driver.spawns.lock().unwrap().clone(),
            1,
            "restored warm VM must be reused after restart — no leak, no respawn"
        );
    }
}
