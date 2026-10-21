//! OfficeCLI gateway routes for the Office add-in.
//!
//! The add-in uploads a snapshot of the live Office document; this module runs
//! the `officecli` binary server-side against that snapshot (read / query /
//! render / validate / mutate / create), serves the produced artifacts back,
//! manages resident (`open`/`save`/`close`) sessions and `watch` preview
//! processes, and exposes the binary's capability surface.
//!
//! The document registry is kept in memory and mirrored to
//! `<office_cli_dir>/docs.json` on every mutation, mirroring the
//! `office_routes.rs` runtime-file pattern.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::AppState;

// ── TTL / limit configuration ────────────────────────────────────────────────

/// Documents idle longer than this are evicted (flushed, closed, deleted).
const DOC_TTL_HOURS: i64 = 24;
/// Resident sessions idle longer than this are closed.
const RESIDENT_IDLE_MINUTES: i64 = 15;
/// Watch processes idle longer than this are killed.
const WATCH_IDLE_MINUTES: i64 = 30;
/// MCP sessions idle longer than this are killed (reaped here too).
const MCP_IDLE_MINUTES: u64 = 15;
/// Default per-command timeout (cap: MAX_EXEC_TIMEOUT_MS).
const DEFAULT_EXEC_TIMEOUT_MS: u64 = 60_000;
const MAX_EXEC_TIMEOUT_MS: u64 = 300_000;
/// Stdout is truncated beyond this many bytes.
const MAX_STDOUT_BYTES: usize = 1024 * 1024;
/// How long the capabilities probe is cached.
const CAPABILITIES_CACHE_SECS: u64 = 300;

/// Commands the gateway will ever pass to the officecli binary.
const ALLOWED_COMMANDS: &[&str] = &[
    "create", "view", "get", "query", "set", "add", "remove", "move", "swap", "validate", "batch",
    "dump", "merge", "raw", "raw-set", "add-part", "refresh", "open", "save", "close", "plugins",
    "load_skill",
];

/// Read/result-producing commands that accept `--json` (everything except the
/// session/lifecycle and file-creation verbs).
const JSON_COMMANDS: &[&str] = &[
    "view", "get", "query", "set", "add", "remove", "move", "swap", "validate", "batch", "dump",
    "merge",
];

// ── Data models ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchInfo {
    pub pid: u32,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfficeCliDoc {
    pub doc_id: Uuid,
    pub user_id: String,
    pub binding_id: Option<String>,
    pub filename: String,
    /// "docx" | "xlsx" | "pptx"
    pub format: String,
    pub path: PathBuf,
    pub size: u64,
    pub created_at: String,
    pub last_access: String,
    pub resident: bool,
    pub watch: Option<WatchInfo>,
}

#[derive(Debug, Deserialize)]
pub struct ExecRequest {
    pub doc_id: Option<String>,
    pub new_filename: Option<String>,
    pub command: String,
    pub path: Option<String>,
    pub props: Option<Map<String, Value>>,
    pub args: Option<Vec<String>>,
    /// Raw JSON string forwarded as `--commands` for `batch`.
    pub commands: Option<String>,
    pub timeout_ms: Option<u64>,
    pub session: Option<bool>,
    pub template_doc_id: Option<String>,
    /// Transport model 3: absolute on-disk path to edit directly instead of an
    /// uploaded snapshot (only honored when the gateway enables live-fs).
    pub live_path: Option<String>,
}

/// Handler-resolved, handler-controlled filesystem locations for a command.
/// `build_argv` never invents paths of its own — every file-producing flag
/// points inside the doc's own directory.
#[derive(Debug, Default)]
pub struct ExecPaths {
    /// Absolute path of the doc the command operates on (when doc_id given).
    pub doc_path: Option<PathBuf>,
    /// Absolute path of the merge template doc (merge only).
    pub template_path: Option<PathBuf>,
    /// Handler-controlled output path for `create` / `merge`.
    pub output_path: Option<PathBuf>,
    /// Handler-controlled `-o` target for `view` html/screenshot and `dump`.
    pub artifact_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct WatchRequest {
    doc_id: String,
}

#[derive(Debug, Deserialize)]
struct WatchProxyPath {
    doc_id: String,
    #[serde(default)]
    rest: Option<String>,
}

struct CachedCapabilities {
    fetched_at: std::time::Instant,
    payload: Value,
}

static CAPABILITIES_CACHE: once_cell::sync::Lazy<RwLock<Option<CachedCapabilities>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

// ── Router ───────────────────────────────────────────────────────────────────

pub fn office_cli_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/office/cli/document",
            post(upload_document).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/office/cli/exec", post(exec_command))
        .route(
            "/office/cli/document/:doc_id/artifact/:name",
            get(download_artifact),
        )
        .route("/office/cli/capabilities", get(get_capabilities))
        .route("/office/cli/watch", post(start_watch))
        .route("/office/cli/watch/:doc_id/proxy", get(watch_proxy))
        .route("/office/cli/watch/:doc_id/proxy/*rest", get(watch_proxy))
        .route("/office/cli/watch/:doc_id", delete(stop_watch))
        .route("/office/cli/mcp", post(crate::office_cli_mcp::mcp_handler))
}

// ── Persistence (mirrors office_routes::save_runtime_file) ───────────────────

fn docs_file_path(config: &AppConfig) -> PathBuf {
    config.office_cli_dir().join("docs.json")
}

/// Load the doc registry from disk (fallback to empty if missing or corrupt).
pub fn load_docs(config: &AppConfig) -> HashMap<Uuid, OfficeCliDoc> {
    let path = docs_file_path(config);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Save the doc registry to disk.
fn save_docs(config: &AppConfig, docs: &HashMap<Uuid, OfficeCliDoc>) -> Result<(), std::io::Error> {
    let path = docs_file_path(config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(docs).unwrap_or_default())
}

// ── Small shared helpers ─────────────────────────────────────────────────────

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

/// Resolve a caller identity the same lenient way `file_routes.rs::caller_id`
/// does: docs are sandboxed per user regardless, so the `"local-dev"`
/// fallback only ever touches that fallback user's own directory.
pub(crate) fn caller_id(headers: &HeaderMap) -> String {
    crate::auth::get_user(headers)
        .map(|u| u.user_id)
        .unwrap_or_else(|| "local-dev".to_string())
}

/// A doc is only ever visible to the user that uploaded it.
fn owned_by(doc: &OfficeCliDoc, user_id: &str) -> bool {
    doc.user_id == user_id
}

/// Look up a doc by id string, enforcing ownership.
fn lookup_doc(
    docs: &HashMap<Uuid, OfficeCliDoc>,
    id: &str,
    user_id: &str,
) -> Result<(Uuid, OfficeCliDoc), (StatusCode, Json<Value>)> {
    let uuid = id
        .parse::<Uuid>()
        .map_err(|_| bad_request("Invalid doc_id"))?;
    let doc = docs
        .get(&uuid)
        .filter(|doc| owned_by(doc, user_id))
        .cloned()
        .ok_or_else(|| not_found("Office CLI document not found"))?;
    Ok((uuid, doc))
}

/// Reduce a client-supplied name to a bare filename: strip any directory
/// components and reject traversal/control characters outright.
fn sanitize_filename(raw: &str) -> Option<String> {
    let name = raw
        .trim()
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or("")
        .trim();
    if name.is_empty() || name == "." || name == ".." || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_string())
}

/// Artifact names must already be bare filenames — no stripping here.
fn validate_artifact_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
}

fn office_format(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    matches!(ext.as_str(), "docx" | "xlsx" | "pptx").then_some(ext)
}

fn doc_dir(doc: &OfficeCliDoc) -> PathBuf {
    doc.path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn content_type_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("html") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("docx") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn artifact_kind(name: &str) -> &'static str {
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") | Some("jpg") | Some("jpeg") => "image",
        Some("html") => "html",
        Some("json") => "json",
        Some("docx") | Some("xlsx") | Some("pptx") => "document",
        Some("txt") => "text",
        _ => "file",
    }
}

fn artifact_entry(doc_id: Uuid, name: &str) -> Value {
    json!({
        "name": name,
        "kind": artifact_kind(name),
        "url": format!("/api/v1/office/cli/document/{}/artifact/{}", doc_id, name),
    })
}

/// First free port in the configured watch range.
fn allocate_watch_port(range: std::ops::RangeInclusive<u16>, in_use: &[u16]) -> Option<u16> {
    range.into_iter().find(|port| !in_use.contains(port))
}

// ── argv construction (pure) ─────────────────────────────────────────────────

/// Prop values are flattened to `--prop k=v`; strings pass through as-is,
/// numbers/bools render bare, anything else is JSON-encoded. Every value is a
/// single argv item — argv is an array, never a shell string, so shell
/// metacharacters in values cannot escape.
fn stringify_prop_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// NOTE: officecli's exact positional/flag ordering must be verified against
/// the installed binary; because argv is always an array, ordering tweaks are
/// safe to make here without touching call sites.
pub fn build_argv(req: &ExecRequest, paths: &ExecPaths) -> Result<Vec<String>, String> {
    let command = req.command.as_str();
    if !ALLOWED_COMMANDS.contains(&command) {
        return Err(format!("Command '{}' is not allowed", command));
    }

    let mut argv: Vec<String> = vec![command.to_string()];

    // Positional document path(s), then the in-document path selector.
    // officecli addressing is positional throughout: `<cmd> <file> [path]
    // [flags]` — there is no `--path` flag (verified against officecli
    // 1.0.138). `merge` is `merge <template> <output> --data <json>`.
    match command {
        "create" => {
            let out = paths
                .output_path
                .as_ref()
                .ok_or("create requires an output path")?;
            argv.push(out.to_string_lossy().to_string());
        }
        "plugins" | "load_skill" => {}
        "merge" => {
            let template = paths
                .template_path
                .as_ref()
                .or(paths.doc_path.as_ref())
                .ok_or("merge requires a template document")?;
            argv.push(template.to_string_lossy().to_string());
            let out = paths
                .output_path
                .as_ref()
                .ok_or("merge requires an output path")?;
            argv.push(out.to_string_lossy().to_string());
        }
        _ => {
            if let Some(doc) = paths.doc_path.as_ref() {
                argv.push(doc.to_string_lossy().to_string());
            }
            // Only path-addressing commands take an in-document selector
            // (`query` sends its selector via `args`; view/validate/batch/
            // session verbs take none).
            const PATH_COMMANDS: &[&str] = &[
                "get", "set", "add", "remove", "move", "swap", "raw", "raw-set", "dump", "add-part",
            ];
            if PATH_COMMANDS.contains(&command) {
                if let Some(path) = req.path.as_ref() {
                    argv.push(path.clone());
                }
            }
        }
    }

    if let Some(props) = req.props.as_ref() {
        for (key, value) in props {
            argv.push("--prop".to_string());
            argv.push(format!("{}={}", key, stringify_prop_value(value)));
        }
    }

    if let Some(args) = req.args.as_ref() {
        argv.extend(args.iter().cloned());
    }

    if command == "batch" {
        let commands = req
            .commands
            .as_ref()
            .ok_or("batch requires a 'commands' JSON string")?;
        // Forwarded verbatim as a single argv item.
        argv.push("--commands".to_string());
        argv.push(commands.clone());
    }

    if let Some(artifact) = paths.artifact_path.as_ref() {
        argv.push("-o".to_string());
        argv.push(artifact.to_string_lossy().to_string());
    }

    if JSON_COMMANDS.contains(&command) {
        argv.push("--json".to_string());
    }

    Ok(argv)
}

// ── Process execution (mirrors tool_routes::shell_exec) ──────────────────────

struct CliOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    truncated: bool,
}

async fn run_officecli(
    config: &AppConfig,
    cwd: &std::path::Path,
    argv: &[String],
    timeout: Duration,
) -> Result<CliOutput, (StatusCode, Json<Value>)> {
    let mut cmd = tokio::process::Command::new(config.officecli_bin());
    cmd.args(argv)
        .current_dir(cwd)
        .env("OFFICECLI_SKIP_UPDATE", "1")
        // Flush mutations to disk before the process returns so artifact
        // downloads and follow-up non-officecli reads never see stale bytes.
        .env("OFFICECLI_RESIDENT_FLUSH", "each")
        .kill_on_drop(true);

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| {
            internal_error(format!(
                "officecli timed out after {}ms",
                timeout.as_millis()
            ))
        })?
        .map_err(|e| internal_error(format!("Failed to spawn officecli: {}", e)))?;

    let (stdout, truncated) = truncate_bytes(&output.stdout, MAX_STDOUT_BYTES);
    Ok(CliOutput {
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
        truncated,
    })
}

fn truncate_bytes(bytes: &[u8], cap: usize) -> (Vec<u8>, bool) {
    if bytes.len() > cap {
        (bytes[..cap].to_vec(), true)
    } else {
        (bytes.to_vec(), false)
    }
}

/// Render an exec-style response. Non-zero exits keep HTTP 200: officecli's
/// structured error (`code`, `suggestion`) is data for the model to
/// self-correct, not a transport failure.
fn exec_response(output: CliOutput, artifacts: Vec<Value>, started: std::time::Instant) -> Value {
    let trimmed = output.stdout.trim();
    let parsed = serde_json::from_str::<Value>(trimmed).ok();
    json!({
        "ok": output.exit_code == 0,
        "exit_code": output.exit_code,
        // officecli's JSON (including its structured errors) passes through untouched.
        "result": parsed.clone().unwrap_or(Value::Null),
        "stdout": if parsed.is_some() || output.stdout.is_empty() {
            Value::Null
        } else {
            Value::String(output.stdout)
        },
        "stderr": output.stderr,
        "artifacts": artifacts,
        "duration_ms": started.elapsed().as_millis() as u64,
        "truncated": output.truncated,
    })
}

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn upload_document(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = caller_id(&headers);
    let raw_name = headers
        .get("x-office-filename")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad_request("Missing required header: x-office-filename"))?;
    let filename =
        sanitize_filename(raw_name).ok_or_else(|| bad_request("Invalid x-office-filename"))?;
    let format = office_format(&filename)
        .ok_or_else(|| bad_request("Unsupported file extension: expected .docx, .xlsx or .pptx"))?;
    // x-office-host is accepted for forward compatibility (not persisted).
    let binding_id = headers
        .get("x-office-binding-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let doc_id = Uuid::new_v4();
    let dir = state
        .config
        .office_cli_dir()
        .join(&user_id)
        .join(doc_id.to_string());
    tokio::fs::create_dir_all(&dir).await.map_err(internal_error)?;
    let path = dir.join(&filename);
    let size = body.len() as u64;
    tokio::fs::write(&path, &body).await.map_err(internal_error)?;

    let now = now_iso();
    let doc = OfficeCliDoc {
        doc_id,
        user_id,
        binding_id,
        filename,
        format: format.clone(),
        path,
        size,
        created_at: now.clone(),
        last_access: now,
        resident: false,
        watch: None,
    };
    let mut docs = state.office_cli_docs.write().await;
    docs.insert(doc_id, doc);
    let _ = save_docs(&state.config, &docs);

    Ok(Json(json!({
        "ok": true,
        "doc_id": doc_id,
        "size": size,
        "format": format,
    })))
}

async fn exec_command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ExecRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let started = std::time::Instant::now();
    let user_id = caller_id(&headers);
    let command = req.command.as_str();

    if !ALLOWED_COMMANDS.contains(&command) {
        return Err(bad_request(format!("Command '{}' is not allowed", command)));
    }

    // ── Target validation: exactly one of doc_id / new_filename ──────────
    match command {
        "create" => {
            if req.doc_id.is_some() || req.new_filename.is_none() {
                return Err(bad_request("create requires new_filename (and no doc_id)"));
            }
        }
        "merge" => {
            if req.new_filename.is_none()
                || (req.doc_id.is_none() && req.template_doc_id.is_none())
            {
                return Err(bad_request(
                    "merge requires new_filename plus doc_id and/or template_doc_id",
                ));
            }
        }
        "plugins" | "load_skill" => {
            if req.doc_id.is_some() || req.new_filename.is_some() {
                return Err(bad_request(format!("{} takes no document target", command)));
            }
        }
        _ => {
            if (req.doc_id.is_none() && req.live_path.is_none()) || req.new_filename.is_some() {
                return Err(bad_request("Pass exactly one of doc_id, new_filename or live_path"));
            }
            if req.doc_id.is_some() && req.live_path.is_some() {
                return Err(bad_request("Pass either doc_id or live_path, not both"));
            }
        }
    }

    // ── Resolve docs and handler-controlled paths ─────────────────────────
    let mut paths = ExecPaths::default();
    let mut working_doc_id: Option<Uuid> = None;
    let mut working_dir = state.config.office_cli_dir();
    let mut created_doc: Option<(Uuid, String, String)> = None;

    // ── Transport model 3: direct on-disk editing (live-fs gateways only) ──
    if let Some(live) = req.live_path.as_deref() {
        if !state.config.officecli_live_fs() {
            return Err(bad_request(
                "live_path is disabled on this gateway (ALLTERNIT_OFFICECLI_LIVE_FS is off)",
            ));
        }
        // Artifact-producing commands need a registry doc for download URLs —
        // use the snapshot (doc_id) flow for those. (`view` always targets an
        // artifact in this design: html by default, png for screenshots.)
        if matches!(command, "create" | "merge" | "dump" | "view") {
            return Err(bad_request(format!(
                "live_path is not supported for '{}'; upload a snapshot and use doc_id",
                command
            )));
        }
        let live_path = std::path::PathBuf::from(live);
        if !live_path.is_absolute() || live.split('/').any(|seg| seg == "..") {
            return Err(bad_request("live_path must be an absolute path without '..'"));
        }
        let filename = live_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if office_format(filename).is_none() {
            return Err(bad_request("live_path must point to a .docx, .xlsx or .pptx file"));
        }
        if !live_path.is_file() {
            return Err(bad_request(format!("live_path does not exist: {}", live)));
        }
        paths.doc_path = Some(live_path.clone());
        working_dir = live_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| state.config.office_cli_dir());
    }

    {
        let mut docs = state.office_cli_docs.write().await;

        if let Some(id) = req.doc_id.as_ref() {
            let (uuid, doc) = lookup_doc(&docs, id, &user_id)?;
            working_doc_id = Some(uuid);
            paths.doc_path = Some(doc.path.clone());
            working_dir = doc_dir(&doc);
        }
        if command == "merge" {
            if let Some(id) = req.template_doc_id.as_ref() {
                let (uuid, doc) = lookup_doc(&docs, id, &user_id)?;
                paths.template_path = Some(doc.path.clone());
                if working_doc_id.is_none() {
                    working_doc_id = Some(uuid);
                    working_dir = doc_dir(&doc);
                }
            }
            let filename = sanitize_filename(req.new_filename.as_deref().unwrap_or(""))
                .ok_or_else(|| bad_request("Invalid new_filename"))?;
            office_format(&filename).ok_or_else(|| {
                bad_request("Unsupported file extension: expected .docx, .xlsx or .pptx")
            })?;
            paths.output_path = Some(working_dir.join("artifacts").join(&filename));
        }
        if command == "create" {
            let filename = sanitize_filename(req.new_filename.as_deref().unwrap_or(""))
                .ok_or_else(|| bad_request("Invalid new_filename"))?;
            let format = office_format(&filename).ok_or_else(|| {
                bad_request("Unsupported file extension: expected .docx, .xlsx or .pptx")
            })?;
            // `create` has no source doc: a fresh doc directory is allocated
            // and registered as a new doc once the command succeeds, so the
            // produced file is downloadable and targetable by later execs.
            let uuid = Uuid::new_v4();
            working_doc_id = Some(uuid);
            working_dir = state
                .config
                .office_cli_dir()
                .join(&user_id)
                .join(uuid.to_string());
            paths.output_path = Some(working_dir.join(&filename));
            created_doc = Some((uuid, filename, format));
        }
        if command == "view" {
            let ext = view_artifact_extension(&req);
            let stamp = Utc::now().format("%Y%m%d%H%M%S");
            paths.artifact_path = Some(
                working_dir
                    .join("artifacts")
                    .join(format!("{}-page.{}", stamp, ext)),
            );
        }
        if command == "dump" {
            let stamp = Utc::now().format("%Y%m%d%H%M%S");
            paths.artifact_path = Some(
                working_dir
                    .join("artifacts")
                    .join(format!("{}-dump.json", stamp)),
            );
        }

        // Touch last_access for the target doc.
        if let Some(id) = req.doc_id.as_ref() {
            if let Ok(uuid) = id.parse::<Uuid>() {
                if let Some(doc) = docs.get_mut(&uuid) {
                    doc.last_access = now_iso();
                }
            }
        }
        let _ = save_docs(&state.config, &docs);
    }

    // ── Resident mode: ensure the doc is open before the real command ─────
    if req.session.unwrap_or(false) {
        if let (Some(uuid), Some(doc_path)) = (working_doc_id, paths.doc_path.clone()) {
            let already_resident = state
                .office_cli_docs
                .read()
                .await
                .get(&uuid)
                .map(|doc| doc.resident)
                .unwrap_or(false);
            if !already_resident {
                let open_argv = vec![
                    "open".to_string(),
                    doc_path.to_string_lossy().to_string(),
                ];
                let open =
                    run_officecli(&state.config, &working_dir, &open_argv, Duration::from_secs(30))
                        .await?;
                if open.exit_code != 0 {
                    return Ok(Json(exec_response(open, Vec::new(), started)));
                }
                let mut docs = state.office_cli_docs.write().await;
                if let Some(doc) = docs.get_mut(&uuid) {
                    doc.resident = true;
                }
                let _ = save_docs(&state.config, &docs);
            }
        }
    }

    // ── Prepare output directories ────────────────────────────────────────
    if command == "create" {
        tokio::fs::create_dir_all(&working_dir)
            .await
            .map_err(internal_error)?;
    }
    if paths.artifact_path.is_some() || command == "merge" {
        tokio::fs::create_dir_all(working_dir.join("artifacts"))
            .await
            .map_err(internal_error)?;
    }

    // ── Run ───────────────────────────────────────────────────────────────
    let argv = build_argv(&req, &paths).map_err(bad_request)?;
    let timeout = Duration::from_millis(
        req.timeout_ms
            .unwrap_or(DEFAULT_EXEC_TIMEOUT_MS)
            .min(MAX_EXEC_TIMEOUT_MS),
    );
    let output = run_officecli(&state.config, &working_dir, &argv, timeout).await?;
    let ok = output.exit_code == 0;

    // ── Collect artifacts produced by this call ───────────────────────────
    let mut artifacts: Vec<Value> = Vec::new();
    if ok {
        for candidate in [&paths.artifact_path, &paths.output_path].into_iter().flatten() {
            if let Some(name) = candidate.file_name().and_then(|n| n.to_str()) {
                if tokio::fs::metadata(candidate)
                    .await
                    .map(|m| m.is_file())
                    .unwrap_or(false)
                {
                    if let Some(uuid) = working_doc_id {
                        artifacts.push(artifact_entry(uuid, name));
                    }
                }
            }
        }
    }

    // ── Registry bookkeeping ──────────────────────────────────────────────
    {
        let mut docs = state.office_cli_docs.write().await;
        if ok {
            if let Some((uuid, filename, format)) = created_doc {
                let path = paths.output_path.clone().unwrap_or_default();
                let size = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                let now = now_iso();
                docs.insert(
                    uuid,
                    OfficeCliDoc {
                        doc_id: uuid,
                        user_id: user_id.clone(),
                        binding_id: None,
                        filename,
                        format,
                        path,
                        size,
                        created_at: now.clone(),
                        last_access: now,
                        resident: false,
                        watch: None,
                    },
                );
            }
            if let Some(uuid) = working_doc_id {
                if let Some(doc) = docs.get_mut(&uuid) {
                    doc.last_access = now_iso();
                    match command {
                        "open" => doc.resident = true,
                        "close" => doc.resident = false,
                        _ => {}
                    }
                    if let Ok(meta) = std::fs::metadata(&doc.path) {
                        doc.size = meta.len();
                    }
                }
            }
        }
        let _ = save_docs(&state.config, &docs);
    }

    Ok(Json(exec_response(output, artifacts, started)))
}

/// `view` produces HTML by default and a PNG for screenshot renders.
fn view_artifact_extension(req: &ExecRequest) -> &'static str {
    let wants_screenshot = req
        .args
        .as_ref()
        .map(|args| args.iter().any(|a| a.contains("screenshot")))
        .unwrap_or(false)
        || req
            .props
            .as_ref()
            .and_then(|props| props.get("format"))
            .and_then(|v| v.as_str())
            .map(|f| f.contains("screenshot") || f == "png")
            .unwrap_or(false);
    if wants_screenshot {
        "png"
    } else {
        "html"
    }
}

async fn download_artifact(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((doc_id, name)): Path<(String, String)>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let user_id = caller_id(&headers);
    if !validate_artifact_name(&name) {
        return Err(not_found("Artifact not found"));
    }
    let uuid = doc_id
        .parse::<Uuid>()
        .map_err(|_| not_found("Office CLI document not found"))?;

    let (doc_path, doc_filename, resident, working_dir) = {
        let docs = state.office_cli_docs.read().await;
        let doc = docs
            .get(&uuid)
            .filter(|doc| owned_by(doc, &user_id))
            .ok_or_else(|| not_found("Office CLI document not found"))?;
        (
            doc.path.clone(),
            doc.filename.clone(),
            doc.resident,
            doc_dir(doc),
        )
    };

    // Flush the resident session before serving the doc's own source file.
    if resident && name == doc_filename {
        let save_argv = vec![
            "save".to_string(),
            doc_path.to_string_lossy().to_string(),
        ];
        let _ = run_officecli(&state.config, &working_dir, &save_argv, Duration::from_secs(30)).await;
    }

    let path = if name == doc_filename {
        working_dir.join(&name)
    } else {
        working_dir.join("artifacts").join(&name)
    };
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| not_found("Artifact not found"))?;
    Ok(([(header::CONTENT_TYPE, content_type_for(&name))], bytes).into_response())
}

async fn get_capabilities(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    {
        let cache = CAPABILITIES_CACHE.read().await;
        if let Some(cached) = cache.as_ref() {
            if cached.fetched_at.elapsed() < Duration::from_secs(CAPABILITIES_CACHE_SECS) {
                return Ok(Json(cached.payload.clone()));
            }
        }
    }

    let probe = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(state.config.officecli_bin())
            .arg("--version")
            .output(),
    )
    .await;
    let (available, version) = match probe {
        Ok(Ok(out)) if out.status.success() => (
            true,
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        ),
        _ => (false, None),
    };

    let payload = json!({
        "ok": true,
        "available": available,
        "version": version,
        "commands": ALLOWED_COMMANDS,
        "live_fs": state.config.officecli_live_fs(),
    });
    *CAPABILITIES_CACHE.write().await = Some(CachedCapabilities {
        fetched_at: std::time::Instant::now(),
        payload: payload.clone(),
    });
    Ok(Json(payload))
}

async fn start_watch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<WatchRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = caller_id(&headers);
    let uuid = req
        .doc_id
        .parse::<Uuid>()
        .map_err(|_| bad_request("Invalid doc_id"))?;

    let (doc_path, working_dir, existing) = {
        let docs = state.office_cli_docs.read().await;
        let doc = docs
            .get(&uuid)
            .filter(|doc| owned_by(doc, &user_id))
            .ok_or_else(|| not_found("Office CLI document not found"))?;
        (doc.path.clone(), doc_dir(doc), doc.watch.clone())
    };

    if let Some(watch) = existing {
        return Ok(Json(watch_response(uuid, watch.port)));
    }

    let port = {
        let docs = state.office_cli_docs.read().await;
        let in_use: Vec<u16> = docs
            .values()
            .filter_map(|doc| doc.watch.as_ref().map(|watch| watch.port))
            .collect();
        allocate_watch_port(state.config.officecli_watch_ports(), &in_use)
            .ok_or_else(|| internal_error("No free ports in ALLTERNIT_OFFICECLI_WATCH_PORTS range"))?
    };

    // NOTE: verify the --port flag against the installed binary
    // (`officecli watch --help`); if unsupported, drop these two args and rely
    // on officecli's default port (only safe while a single watch is active).
    let mut cmd = tokio::process::Command::new(state.config.officecli_bin());
    cmd.arg("watch")
        .arg(&doc_path)
        .arg("--port")
        .arg(port.to_string())
        .current_dir(&working_dir)
        .env("OFFICECLI_SKIP_UPDATE", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let child = cmd
        .spawn()
        .map_err(|e| internal_error(format!("Failed to spawn officecli watch: {}", e)))?;
    let pid = child.id().unwrap_or(0);

    state.office_cli_watches.write().await.insert(uuid, child);

    let mut docs = state.office_cli_docs.write().await;
    if let Some(doc) = docs.get_mut(&uuid) {
        doc.watch = Some(WatchInfo { pid, port });
        doc.last_access = now_iso();
    }
    let _ = save_docs(&state.config, &docs);

    Ok(Json(watch_response(uuid, port)))
}

fn watch_response(doc_id: Uuid, port: u16) -> Value {
    json!({
        "ok": true,
        "watch_url": format!("http://127.0.0.1:{}", port),
        "port": port,
        // For remote-gateway mode the add-in uses this proxied URL instead.
        "proxy_url": format!("/api/v1/office/cli/watch/{}/proxy", doc_id),
    })
}

/// Best-effort plain-GET reverse proxy for remote-gateway mode. If the watch
/// page uses WebSocket for auto-refresh, live-refresh degrades to manual
/// reload through this proxy (documented limitation; upgrade to WS if the
/// real binary requires it).
async fn watch_proxy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<WatchProxyPath>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let user_id = caller_id(&headers);
    let uuid = path
        .doc_id
        .parse::<Uuid>()
        .map_err(|_| not_found("Office CLI watch not found"))?;
    let port = {
        let docs = state.office_cli_docs.read().await;
        docs.get(&uuid)
            .filter(|doc| owned_by(doc, &user_id))
            .and_then(|doc| doc.watch.as_ref().map(|watch| watch.port))
            .ok_or_else(|| not_found("Office CLI watch not found"))?
    };

    let rest = path.rest.unwrap_or_default();
    let url = format!("http://127.0.0.1:{}/{}", port, rest);
    let upstream = reqwest::get(&url).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "Watch proxy error", "message": e.to_string() })),
        )
    })?;
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::OK);
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/html")
        .to_string();
    let body = upstream.bytes().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "Watch proxy error", "message": e.to_string() })),
        )
    })?;
    Ok((status, [(header::CONTENT_TYPE, content_type)], body).into_response())
}

async fn stop_watch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(doc_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let user_id = caller_id(&headers);
    let uuid = doc_id
        .parse::<Uuid>()
        .map_err(|_| not_found("Office CLI document not found"))?;
    {
        let docs = state.office_cli_docs.read().await;
        docs.get(&uuid)
            .filter(|doc| owned_by(doc, &user_id))
            .ok_or_else(|| not_found("Office CLI document not found"))?;
    }

    if let Some(mut child) = state.office_cli_watches.write().await.remove(&uuid) {
        let _ = child.kill().await;
    }

    let mut docs = state.office_cli_docs.write().await;
    if let Some(doc) = docs.get_mut(&uuid) {
        doc.watch = None;
        doc.last_access = now_iso();
    }
    let _ = save_docs(&state.config, &docs);

    Ok(Json(json!({ "ok": true })))
}

// ── Idle reaper (spawned from main; mirrors cleanup_stale_data TTL style) ────

/// Periodically evicts idle docs (flush + close residents, kill watches,
/// delete the doc dir), closes idle resident sessions, kills idle watch
/// processes and idle MCP sessions.
pub async fn reap_idle_sessions(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        reap_once(&state).await;
    }
}

async fn reap_once(state: &Arc<AppState>) {
    let now = Utc::now();
    let snapshot: Vec<OfficeCliDoc> = state.office_cli_docs.read().await.values().cloned().collect();
    let mut changed = false;

    for doc in snapshot {
        let idle = DateTime::parse_from_rfc3339(&doc.last_access)
            .ok()
            .map(|seen| now.signed_duration_since(seen.with_timezone(&Utc)));
        let Some(idle) = idle else { continue };
        let working_dir = doc_dir(&doc);

        if idle.num_hours() >= DOC_TTL_HOURS {
            // Flush and close before removing anything on disk.
            if doc.resident {
                let save_argv = vec!["save".to_string(), doc.path.to_string_lossy().to_string()];
                let _ =
                    run_officecli(&state.config, &working_dir, &save_argv, Duration::from_secs(15))
                        .await;
                let close_argv =
                    vec!["close".to_string(), doc.path.to_string_lossy().to_string()];
                let _ =
                    run_officecli(&state.config, &working_dir, &close_argv, Duration::from_secs(15))
                        .await;
            }
            if doc.watch.is_some() {
                if let Some(mut child) =
                    state.office_cli_watches.write().await.remove(&doc.doc_id)
                {
                    let _ = child.kill().await;
                }
            }
            let _ = std::fs::remove_dir_all(&working_dir);
            state.office_cli_docs.write().await.remove(&doc.doc_id);
            changed = true;
            continue;
        }

        if doc.resident && idle.num_minutes() >= RESIDENT_IDLE_MINUTES {
            let close_argv = vec!["close".to_string(), doc.path.to_string_lossy().to_string()];
            let _ = run_officecli(&state.config, &working_dir, &close_argv, Duration::from_secs(15))
                .await;
            if let Some(entry) = state.office_cli_docs.write().await.get_mut(&doc.doc_id) {
                entry.resident = false;
            }
            changed = true;
        }
        if doc.watch.is_some() && idle.num_minutes() >= WATCH_IDLE_MINUTES {
            if let Some(mut child) = state.office_cli_watches.write().await.remove(&doc.doc_id) {
                let _ = child.kill().await;
            }
            if let Some(entry) = state.office_cli_docs.write().await.get_mut(&doc.doc_id) {
                entry.watch = None;
            }
            changed = true;
        }
    }

    if changed {
        let docs = state.office_cli_docs.read().await;
        let _ = save_docs(&state.config, &docs);
    }

    // Kill MCP sessions idle past their TTL.
    let mut sessions = state.office_cli_mcp_sessions.write().await;
    let idle_users: Vec<String> = sessions
        .iter()
        .filter(|(_, session)| {
            session.last_active.elapsed() > Duration::from_secs(MCP_IDLE_MINUTES * 60)
        })
        .map(|(user, _)| user.clone())
        .collect();
    for user in idle_users {
        if let Some(mut session) = sessions.remove(&user) {
            session.shutdown().await;
        }
    }
}

// ── Error envelopes (mirror office_routes) ───────────────────────────────────

fn internal_error<E: std::fmt::Display>(error: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "error": "Internal server error",
            "message": error.to_string(),
        })),
    )
}

fn bad_request<E: std::fmt::Display>(message: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "Bad request",
            "message": message.to_string(),
        })),
    )
}

fn not_found<E: std::fmt::Display>(message: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": message.to_string() })),
    )
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn exec_req(command: &str) -> ExecRequest {
        ExecRequest {
            doc_id: Some(Uuid::new_v4().to_string()),
            new_filename: None,
            command: command.to_string(),
            path: None,
            props: None,
            args: None,
            commands: None,
            timeout_ms: None,
            session: None,
            template_doc_id: None,
            live_path: None,
        }
    }

    fn doc_paths() -> ExecPaths {
        ExecPaths {
            doc_path: Some(PathBuf::from("/tmp/docs/abc/report.docx")),
            template_path: None,
            output_path: None,
            artifact_path: None,
        }
    }

    // ── build_argv ────────────────────────────────────────────────────────

    #[test]
    fn build_argv_rejects_unknown_command() {
        assert!(build_argv(&exec_req("exec"), &doc_paths()).is_err());
        assert!(build_argv(&exec_req("$(rm -rf /)"), &doc_paths()).is_err());
        assert!(build_argv(&exec_req("view; rm -rf /"), &doc_paths()).is_err());
    }

    #[test]
    fn build_argv_positions_doc_path() {
        let argv = build_argv(&exec_req("get"), &doc_paths()).unwrap();
        assert_eq!(argv[0], "get");
        assert_eq!(argv[1], "/tmp/docs/abc/report.docx");
    }

    #[test]
    fn build_argv_path_selector_is_positional_not_a_flag() {
        let mut req = exec_req("get");
        req.path = Some("/slide[1]".to_string());
        let argv = build_argv(&req, &doc_paths()).unwrap();
        // `get <file> <path>` — officecli has no `--path` flag.
        assert_eq!(argv[2], "/slide[1]");
        assert!(!argv.contains(&"--path".to_string()));

        // `view` takes no in-document selector — a stray path is ignored.
        let mut req = exec_req("view");
        req.path = Some("/slide[1]".to_string());
        let argv = build_argv(&req, &doc_paths()).unwrap();
        assert!(!argv.contains(&"/slide[1]".to_string()));
    }

    #[test]
    fn build_argv_flattens_props_as_single_items() {
        let mut req = exec_req("set");
        let mut props = Map::new();
        props.insert("text".to_string(), Value::String("hello".to_string()));
        props.insert("count".to_string(), json!(3));
        props.insert("bold".to_string(), json!(true));
        props.insert(
            "evil".to_string(),
            Value::String("a; rm -rf /".to_string()),
        );
        req.props = Some(props);
        let argv = build_argv(&req, &doc_paths()).unwrap();
        // Every prop is exactly two argv items: "--prop" and "k=v".
        let prop_pos = |needle: &str| {
            argv.iter()
                .position(|a| a == needle)
                .unwrap_or_else(|| panic!("missing {}", needle))
        };
        let i = prop_pos("text=hello");
        assert_eq!(argv[i - 1], "--prop");
        assert!(argv.contains(&"count=3".to_string()));
        assert!(argv.contains(&"bold=true".to_string()));
        // The injection string stays ONE argv item — no splitting, no shell.
        assert!(argv.contains(&"evil=a; rm -rf /".to_string()));
    }

    #[test]
    fn build_argv_json_flag_only_for_read_style_commands() {
        for cmd in [
            "view", "get", "query", "set", "add", "remove", "move", "swap", "validate", "dump",
            "batch", "merge",
        ] {
            let mut req = exec_req(cmd);
            if cmd == "batch" {
                req.commands = Some("[]".to_string());
            }
            if cmd == "merge" {
                req.new_filename = Some("out.docx".to_string());
            }
            let paths = ExecPaths {
                output_path: (cmd == "merge").then(|| PathBuf::from("/tmp/docs/abc/artifacts/out.docx")),
                ..doc_paths()
            };
            let argv = build_argv(&req, &paths).unwrap();
            assert!(argv.contains(&"--json".to_string()), "{} should get --json", cmd);
        }
        for cmd in ["open", "save", "close"] {
            let argv = build_argv(&exec_req(cmd), &doc_paths()).unwrap();
            assert!(!argv.contains(&"--json".to_string()), "{} must not get --json", cmd);
        }
        let mut create_req = exec_req("create");
        create_req.doc_id = None;
        create_req.new_filename = Some("new.pptx".to_string());
        let paths = ExecPaths {
            output_path: Some(PathBuf::from("/tmp/docs/new/new.pptx")),
            ..ExecPaths::default()
        };
        let argv = build_argv(&create_req, &paths).unwrap();
        assert!(!argv.contains(&"--json".to_string()));
        assert_eq!(argv[1], "/tmp/docs/new/new.pptx");
    }

    #[test]
    fn build_argv_batch_commands_passthrough() {
        let mut req = exec_req("batch");
        req.commands = Some(r#"[{"command":"set","props":{"a":"b; rm -rf /"}}]"#.to_string());
        let argv = build_argv(&req, &doc_paths()).unwrap();
        let pos = argv.iter().position(|a| a == "--commands").unwrap();
        // The whole commands JSON is a single argv item, verbatim.
        assert_eq!(
            argv[pos + 1],
            r#"[{"command":"set","props":{"a":"b; rm -rf /"}}]"#
        );
    }

    #[test]
    fn build_argv_view_writes_into_artifacts() {
        let req = exec_req("view");
        let paths = ExecPaths {
            artifact_path: Some(PathBuf::from("/tmp/docs/abc/artifacts/20240101-page.html")),
            ..doc_paths()
        };
        let argv = build_argv(&req, &paths).unwrap();
        let pos = argv.iter().position(|a| a == "-o").unwrap();
        assert_eq!(argv[pos + 1], "/tmp/docs/abc/artifacts/20240101-page.html");
    }

    #[test]
    fn build_argv_merge_uses_template_and_output() {
        let mut req = exec_req("merge");
        req.new_filename = Some("merged.docx".to_string());
        req.template_doc_id = Some(Uuid::new_v4().to_string());
        let paths = ExecPaths {
            doc_path: Some(PathBuf::from("/tmp/docs/abc/report.docx")),
            template_path: Some(PathBuf::from("/tmp/docs/tpl/template.docx")),
            output_path: Some(PathBuf::from("/tmp/docs/abc/artifacts/merged.docx")),
            artifact_path: None,
        };
        let argv = build_argv(&req, &paths).unwrap();
        // `merge <template> <output>` — both positional, no -o flag (verified
        // against officecli 1.0.138). With a separate template given, the doc
        // itself is not a positional.
        assert_eq!(argv[1], "/tmp/docs/tpl/template.docx");
        assert_eq!(argv[2], "/tmp/docs/abc/artifacts/merged.docx");
        assert!(!argv.contains(&"-o".to_string()));
        assert!(!argv.contains(&"/tmp/docs/abc/report.docx".to_string()));
    }

    // ── Filename / artifact validation ─────────────────────────────────────

    #[test]
    fn sanitize_filename_strips_directories() {
        assert_eq!(
            sanitize_filename("../../etc/passwd.docx"),
            Some("passwd.docx".to_string())
        );
        assert_eq!(
            sanitize_filename("C:\\tmp\\report.docx"),
            Some("report.docx".to_string())
        );
        assert_eq!(sanitize_filename("report.docx"), Some("report.docx".to_string()));
        assert_eq!(sanitize_filename(".."), None);
        assert_eq!(sanitize_filename("   "), None);
    }

    #[test]
    fn artifact_names_reject_traversal() {
        assert!(!validate_artifact_name("../docs.json"));
        assert!(!validate_artifact_name("a/b.png"));
        assert!(!validate_artifact_name("a\\b.png"));
        assert!(!validate_artifact_name(".."));
        assert!(!validate_artifact_name(""));
        assert!(validate_artifact_name("20240101-page.png"));
    }

    // ── Ownership / port allocation ────────────────────────────────────────

    fn doc_for(user: &str) -> OfficeCliDoc {
        let now = now_iso();
        OfficeCliDoc {
            doc_id: Uuid::new_v4(),
            user_id: user.to_string(),
            binding_id: None,
            filename: "report.docx".to_string(),
            format: "docx".to_string(),
            path: PathBuf::from("/tmp/docs/abc/report.docx"),
            size: 10,
            created_at: now.clone(),
            last_access: now,
            resident: false,
            watch: None,
        }
    }

    #[test]
    fn docs_are_invisible_to_other_users() {
        let doc = doc_for("user-1");
        let mut docs = HashMap::new();
        docs.insert(doc.doc_id, doc.clone());
        assert!(lookup_doc(&docs, &doc.doc_id.to_string(), "user-1").is_ok());
        // Another user gets the same 404 as a missing doc (no existence leak).
        assert!(lookup_doc(&docs, &doc.doc_id.to_string(), "user-2").is_err());
        assert!(lookup_doc(&docs, &Uuid::new_v4().to_string(), "user-1").is_err());
    }

    #[test]
    fn watch_port_allocation_skips_in_use() {
        let range = 26400..=26402;
        assert_eq!(allocate_watch_port(range.clone(), &[]), Some(26400));
        assert_eq!(allocate_watch_port(range.clone(), &[26400]), Some(26401));
        assert_eq!(allocate_watch_port(range.clone(), &[26400, 26401, 26402]), None);
    }

    // ── Route round-trip with a fake officecli binary ──────────────────────
    // One sequential test: OFFICECLI_BIN / ALLTERNIT_OFFICE_CLI_DIR are
    // process-global env, so everything that depends on them runs here.

    #[tokio::test]
    async fn gateway_round_trip_with_fake_officecli() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = std::env::temp_dir().join(format!("officecli-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let bin = temp.join("fake-officecli");
        let log = temp.join("argv.log");
        std::fs::write(&bin, FAKE_OFFICECLI_SCRIPT).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("OFFICECLI_BIN", &bin);
        std::env::set_var("ALLTERNIT_OFFICE_CLI_DIR", temp.join("office-cli"));
        std::env::set_var("FAKE_OFFICECLI_LOG", &log);
        std::env::set_var("ALLTERNIT_OFFICECLI_WATCH_PORTS", "26500-26502");

        let state = test_app_state(&temp).await;
        let app = office_cli_router().with_state(state.clone());

        // ── Upload ────────────────────────────────────────────────────────
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/document")
                    .header("x-office-filename", "report.docx")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::from("fake-docx-bytes"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response.into_body()).await;
        assert_eq!(body["ok"], json!(true));
        let doc_id = body["doc_id"].as_str().unwrap().to_string();

        // Upload without a filename header is a 400.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/document")
                    .body(Body::from("x"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // ── Exec (resident session opens the doc first) ───────────────────
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/exec")
                    .header("content-type", "application/json")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::from(format!(
                        r#"{{"doc_id":"{}","command":"get","session":true}}"#,
                        doc_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response.into_body()).await;
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["result"]["text"], json!("fake view"));

        // The fake binary saw `open <path>` then `get <path> --json`.
        let log_text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = log_text.lines().collect();
        assert!(lines[0].starts_with("open "), "unexpected log: {}", log_text);
        assert!(lines[1].starts_with("get "), "unexpected log: {}", log_text);
        assert!(lines[1].ends_with("--json"), "unexpected log: {}", log_text);
        assert!(
            state
                .office_cli_docs
                .read()
                .await
                .get(&doc_id.parse::<Uuid>().unwrap())
                .map(|doc| doc.resident)
                .unwrap_or(false)
        );

        // ── Exec view → artifact, then download it ────────────────────────
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/exec")
                    .header("content-type", "application/json")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::from(format!(
                        r#"{{"doc_id":"{}","command":"view","args":["html"]}}"#,
                        doc_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(response.into_body()).await;
        assert_eq!(body["ok"], json!(true));
        let artifact = &body["artifacts"][0];
        let artifact_name = artifact["name"].as_str().unwrap().to_string();
        assert!(artifact_name.ends_with("-page.html"));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/office/cli/document/{}/artifact/{}",
                        doc_id, artifact_name
                    ))
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&bytes).contains("fake"));

        // Traversal is rejected.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/office/cli/document/{}/artifact/..%2fdocs.json", doc_id))
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // ── Ownership isolation ───────────────────────────────────────────
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/exec")
                    .header("content-type", "application/json")
                    .header("x-allternit-user-id", "user-2")
                    .body(Body::from(format!(
                        r#"{{"doc_id":"{}","command":"get"}}"#,
                        doc_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // ── Capabilities ──────────────────────────────────────────────────
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/office/cli/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(response.into_body()).await;
        assert_eq!(body["available"], json!(true));
        assert!(body["version"].as_str().unwrap().contains("fake"));

        // ── Watch start/stop ──────────────────────────────────────────────
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/watch")
                    .header("content-type", "application/json")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::from(format!(r#"{{"doc_id":"{}"}}"#, doc_id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(response.into_body()).await;
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["port"], json!(26500));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/office/cli/watch/{}", doc_id))
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(response.into_body()).await;
        assert_eq!(body["ok"], json!(true));

        // ── MCP passthrough + respawn after child death ───────────────────
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/mcp")
                    .header("content-type", "application/json")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(response.into_body()).await;
        assert_eq!(body["result"]["tools"][0]["name"], json!("docx_get"));

        // Kill the MCP child; the next request must respawn and succeed.
        if let Some(session) = state.office_cli_mcp_sessions.write().await.get_mut("user-1") {
            session.shutdown().await;
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/office/cli/mcp")
                    .header("content-type", "application/json")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"docx_get","arguments":{}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(response.into_body()).await;
        assert_eq!(body["id"], json!(2));
        assert!(body.get("error").is_none(), "respawn failed: {}", body);

        // ── Registry persisted across reload ──────────────────────────────
        let reloaded = load_docs(&state.config);
        assert!(reloaded.contains_key(&doc_id.parse::<Uuid>().unwrap()));

        std::env::remove_var("OFFICECLI_BIN");
        std::env::remove_var("ALLTERNIT_OFFICE_CLI_DIR");
        std::env::remove_var("FAKE_OFFICECLI_LOG");
        std::env::remove_var("ALLTERNIT_OFFICECLI_WATCH_PORTS");
        let _ = std::fs::remove_dir_all(&temp);
    }

    async fn body_json(body: axum::body::Body) -> Value {
        use http_body_util::BodyExt;
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn test_app_state(temp: &std::path::Path) -> Arc<AppState> {
        let config = AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(RwLock::new(crate::office_routes::OfficeRuntimeFile::default())),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
                "__test__".to_string(),
            ),
            fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    /// Fake officecli: logs argv, serves fixture JSON, writes `-o` targets for
    /// `view`, sleeps for `watch`, and speaks newline-delimited JSON-RPC for
    /// `mcp serve`.
    const FAKE_OFFICECLI_SCRIPT: &str = r##"#!/bin/sh
echo "$@" >> "$FAKE_OFFICECLI_LOG"
case "$1" in
  --version)
    echo "officecli 0.0.0-fake"
    exit 0 ;;
  watch)
    sleep 60
    exit 0 ;;
  mcp)
    while IFS= read -r line; do
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *notifications/initialized*)
          : ;;
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}\n' "${id:-0}" ;;
        *'"method":"tools/list"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"docx_get"}]}}\n' "$id" ;;
        *'"method":"tools/call"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"fake-result"}]}}\n' "$id" ;;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "${id:-0}" ;;
      esac
    done
    exit 0 ;;
  view)
    out=""
    prev=""
    for a in "$@"; do
      if [ "$prev" = "-o" ]; then out="$a"; fi
      prev="$a"
    done
    [ -n "$out" ] && printf '<html>fake</html>' > "$out"
    echo '{"text":"fake view"}'
    exit 0 ;;
esac
echo '{"text":"fake view"}'
exit 0
"##;
}
