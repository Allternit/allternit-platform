//! Design / Composio connector routes
//!
//! Also hosts Open Design LTS endpoints:
//! - skill discovery across Claude Desktop, Codex CLI, and Allternit local paths
//! - agent adapter detection and spawn metadata

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::AppState;

/// Daemon-side cache for discovered Open Design skills.
/// Refreshes on a polling interval in dev and can be invalidated by SIGHUP in
/// production so skill discovery is fast and hot-reloads without restarting the
/// API.
#[derive(Debug, Clone)]
pub struct DesignSkillCache {
    inner: Arc<RwLock<DesignSkillCacheInner>>,
}

#[derive(Debug, Clone, Default)]
struct DesignSkillCacheInner {
    skills: Vec<DiscoveredSkill>,
    scanned_paths: Vec<String>,
    total: usize,
    refreshed_at: Option<Instant>,
}

impl DesignSkillCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(DesignSkillCacheInner::default())),
        }
    }

    pub async fn get(&self, cwd: Option<&str>) -> DiscoverSkillsResponse {
        // Return cached result if fresh (< 5s) to avoid filesystem churn.
        let is_fresh = {
            let guard = self.inner.read().await;
            guard
                .refreshed_at
                .map_or(false, |t| t.elapsed() < Duration::from_secs(5))
        };

        if !is_fresh {
            self.refresh(cwd).await;
        }

        let guard = self.inner.read().await;
        DiscoverSkillsResponse {
            skills: guard.skills.clone(),
            scanned_paths: guard.scanned_paths.clone(),
            total: guard.total,
        }
    }

    pub async fn refresh(&self, cwd: Option<&str>) {
        let paths = discover_skills_paths(cwd);
        let mut skills = Vec::new();
        let scanned: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        for path in &paths {
            let source = if path.starts_with(dirs::home_dir().unwrap_or_default().join(".claude")) {
                "claude-desktop"
            } else if path.ends_with("skills") && path.components().count() <= 3 {
                "codex-cli"
            } else {
                "allternit-local"
            };
            scan_skill_directory(path, source, &mut skills);
        }

        let mut guard = self.inner.write().await;
        guard.skills = skills;
        guard.scanned_paths = scanned;
        guard.total = guard.skills.len();
        guard.refreshed_at = Some(Instant::now());
    }
}

pub fn design_connector_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/design/composio/connections",
            get(list_composio_connections),
        )
        .route("/design/composio/connect", post(connect_composio))
        .route("/design/connectors/slack", post(slack_connector))
        .route("/design/connectors/notion", post(notion_connector))
        .route("/design/connectors/linear", post(linear_connector))
        .route("/design/connectors/github", post(github_connector))
        .route("/design/skills/discover", get(discover_skills))
        .route("/design/adapters", get(list_adapters))
        .route("/design/adapters/detect", post(detect_adapters))
        .route("/design/adapters/spawn", post(spawn_adapter))
        .route("/design/plugins/install", post(install_plugin))
        .route("/design/import-url", post(import_url))
}

// ─── Open Design connector catalog (in-process) ───────────────────────────────
// The connector registry is part of Allternit Design, not a separate service.
// We vendor the Open Design connector catalog at build time and serve it
// directly from this process — no daemon, no proxy, Allternit-branded.
//
// Live connect/execute is OWNED: it runs through the in-process connector
// standard at /api/v1/connectors (connector_routes.rs) — local_cli (instant,
// e.g. `gh`), owned oauth2 loopback + device_flow, api_key, and the synthesized
// connector-MCP. No Composio, no third-party key. These legacy /design/* routes
// are kept only as a thin pointer to that owned standard; they never claim a
// connector is live and never ask for a third-party key.

const CONNECTOR_CATALOG_JSON: &str = include_str!("../assets/open-design/connectors.json");

fn connector_catalog() -> &'static serde_json::Value {
    static CATALOG: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(CONNECTOR_CATALOG_JSON).unwrap_or_else(|_| json!({"connectors": []}))
    })
}

fn find_connector(id: &str) -> Option<serde_json::Value> {
    connector_catalog()
        .get("connectors")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|c| c.get("id").and_then(|i| i.as_str()) == Some(id))
                .cloned()
        })
}

async fn list_composio_connections(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let connectors = connector_catalog()
        .get("connectors")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let total = connectors.as_array().map(|a| a.len()).unwrap_or(0);
    Json(json!({ "connectors": connectors, "total": total, "source": "allternit-in-process" }))
}

fn connect_response(id: &str) -> (StatusCode, Json<serde_json::Value>) {
    match find_connector(id) {
        Some(c) => (
            StatusCode::OK,
            Json(json!({
                "status": "use_owned_connect",
                "connector": id,
                "owned": true,
                "owned_endpoint": format!("/api/v1/connectors/{}/connect", id),
                "owned_execute": format!("/api/v1/connectors/{}/execute", id),
                "catalog": c,
                "message": "Allternit owns this connector in-process. Use POST /api/v1/connectors/:id/connect (local_cli instant, owned OAuth one-click) and POST /api/v1/connectors/:id/execute. No Composio, no third-party key."
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown_connector", "connector": id})),
        ),
    }
}

async fn connect_composio(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let id = body
        .get("connector")
        .or_else(|| body.get("connectorId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match id {
        Some(id) => connect_response(&id),
        None => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "connector id is required (pass {\"connector\":\"slack\"})"})),
        ),
    }
}

async fn slack_connector(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    connect_response("slack")
}

async fn notion_connector(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    connect_response("notion")
}

async fn linear_connector(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    connect_response("linear")
}

async fn github_connector(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    connect_response("github")
}

// ─── Skill Discovery ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSkill {
    pub id: String,
    pub name: String,
    pub path: String,
    pub source: String,
    pub manifest: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverSkillsResponse {
    pub skills: Vec<DiscoveredSkill>,
    pub scanned_paths: Vec<String>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverSkillsQuery {
    pub cwd: Option<String>,
}

fn discover_skills_paths(cwd: Option<&str>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(cwd) = cwd {
        let base = Path::new(cwd);
        paths.push(base.join("skills"));
        paths.push(base.join(".claude").join("skills"));
    }

    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".claude").join("skills"));
    }

    paths
}

fn scan_skill_directory(root: &Path, source: &str, out: &mut Vec<DiscoveredSkill>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut name = id.clone();
        let mut manifest_value: Option<serde_json::Value> = None;

        for filename in ["open-design.json", "manifest.json", "claude.json"] {
            let manifest_path = path.join(filename);
            if manifest_path.is_file() {
                if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(n) = parsed.get("name").and_then(|v| v.as_str()) {
                            name = n.to_string();
                        }
                        manifest_value = Some(parsed);
                    }
                }
                break;
            }
        }

        // Fallback: parse SKILL.md frontmatter / first heading
        if manifest_value.is_none() {
            let skill_md = path.join("SKILL.md");
            if let Ok(raw) = std::fs::read_to_string(&skill_md) {
                let in_frontmatter = raw.starts_with("---");
                let mut passed_frontmatter = false;
                for line in raw.lines() {
                    if in_frontmatter && !passed_frontmatter {
                        if line == "---" {
                            passed_frontmatter = true;
                        }
                        continue;
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed.starts_with('#') {
                        name = trimmed.trim_start_matches('#').trim().to_string();
                        break;
                    }
                }
            }
        }

        out.push(DiscoveredSkill {
            id,
            name,
            path: path.to_string_lossy().to_string(),
            source: source.to_string(),
            manifest: manifest_value,
        });
    }
}

async fn discover_skills(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiscoverSkillsQuery>,
) -> impl IntoResponse {
    let response = state.design_skill_cache.get(query.cwd.as_deref()).await;
    Json(response)
}

// ─── Agent Adapter Detection / Spawning ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterKind {
    pub id: String,
    pub name: String,
    pub description: String,
    pub runtime: String,
    pub required_env: Vec<String>,
    pub optional_env: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterDetectionResult {
    pub available: Vec<String>,
    pub missing: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectAdaptersRequest {
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAdapterRequest {
    pub kind: String,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAdapterResponse {
    pub kind: String,
    pub status: String,
    pub command: Option<String>,
    pub pid: Option<u32>,
}

fn adapter_kinds() -> Vec<AdapterKind> {
    vec![
        AdapterKind {
            id: "claude-desktop".to_string(),
            name: "Claude Desktop".to_string(),
            description: "Local Claude Desktop MCP / skills adapter.".to_string(),
            runtime: "mcp".to_string(),
            required_env: vec![],
            optional_env: vec!["CLAUDE_CONFIG_PATH".to_string()],
        },
        AdapterKind {
            id: "codex-cli".to_string(),
            name: "Codex CLI".to_string(),
            description: "OpenAI Codex CLI skill adapter.".to_string(),
            runtime: "subprocess".to_string(),
            required_env: vec!["OPENAI_API_KEY".to_string()],
            optional_env: vec!["CODEX_SKILLS_PATH".to_string()],
        },
        AdapterKind {
            id: "cursor-agent".to_string(),
            name: "Cursor Agent".to_string(),
            description: "Cursor agent CLI adapter.".to_string(),
            runtime: "subprocess".to_string(),
            required_env: vec![],
            optional_env: vec!["CURSOR_API_KEY".to_string()],
        },
        AdapterKind {
            id: "kimi-cli".to_string(),
            name: "Kimi CLI".to_string(),
            description: "Moonshot Kimi agent CLI adapter.".to_string(),
            runtime: "subprocess".to_string(),
            required_env: vec![],
            optional_env: vec!["KIMI_API_KEY".to_string()],
        },
        AdapterKind {
            id: "allternit-local".to_string(),
            name: "Allternit Local".to_string(),
            description: "In-process Allternit design runtime adapter.".to_string(),
            runtime: "local".to_string(),
            required_env: vec![],
            optional_env: vec!["ALLTERNIT_LOCAL_URL".to_string()],
        },
        AdapterKind {
            id: "generic-mcp".to_string(),
            name: "Generic MCP".to_string(),
            description: "Stdio/SSE Model Context Protocol server.".to_string(),
            runtime: "subprocess".to_string(),
            required_env: vec!["MCP_COMMAND".to_string()],
            optional_env: vec!["MCP_CWD".to_string()],
        },
    ]
}

async fn list_adapters(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "adapters": adapter_kinds(),
        "total": adapter_kinds().len(),
    }))
}

fn command_on_path(cmd: &str) -> Option<PathBuf> {
    let path_env = std::env::var("PATH").unwrap_or_default();
    for dir in path_env.split(':') {
        let candidate = Path::new(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

async fn detect_adapters(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<DetectAdaptersRequest>,
) -> impl IntoResponse {
    let mut available = Vec::new();
    let mut missing = Vec::new();
    let mut env = HashMap::new();

    let home = dirs::home_dir().unwrap_or_default();
    let cwd = req.cwd.as_deref().map(Path::new);

    // Claude Desktop
    if home.join(".claude").is_dir() || command_on_path("claude").is_some() {
        available.push("claude-desktop".to_string());
        env.insert(
            "CLAUDE_CONFIG_PATH".to_string(),
            home.join(".claude")
                .join("claude_desktop_config.json")
                .to_string_lossy()
                .to_string(),
        );
    } else {
        missing.push("claude-desktop".to_string());
    }

    // Codex CLI
    if command_on_path("codex").is_some() {
        available.push("codex-cli".to_string());
        if let Ok(v) = std::env::var("OPENAI_API_KEY") {
            env.insert("OPENAI_API_KEY".to_string(), v);
        }
    } else {
        missing.push("codex-cli".to_string());
    }

    // Cursor agent
    if command_on_path("cursor-agent").is_some() || home.join(".cursor").is_dir() {
        available.push("cursor-agent".to_string());
    } else {
        missing.push("cursor-agent".to_string());
    }

    // Kimi CLI
    if command_on_path("kimi").is_some() {
        available.push("kimi-cli".to_string());
    } else {
        missing.push("kimi-cli".to_string());
    }

    // Allternit local
    if cwd.map(|p| p.join(".allternit").is_dir()).unwrap_or(false) {
        available.push("allternit-local".to_string());
    } else {
        missing.push("allternit-local".to_string());
    }

    // Generic MCP
    if std::env::var("MCP_COMMAND").is_ok() {
        available.push("generic-mcp".to_string());
        if let Ok(v) = std::env::var("MCP_COMMAND") {
            env.insert("MCP_COMMAND".to_string(), v);
        }
    } else {
        missing.push("generic-mcp".to_string());
    }

    Json(AdapterDetectionResult {
        available,
        missing,
        env,
    })
}

async fn spawn_adapter(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<SpawnAdapterRequest>,
) -> impl IntoResponse {
    let kinds: HashMap<String, AdapterKind> = adapter_kinds()
        .into_iter()
        .map(|k| (k.id.clone(), k))
        .collect();

    let Some(kind) = kinds.get(&req.kind) else {
        return Json(json!({
            "error": format!("Unknown adapter kind: {}", req.kind),
        }));
    };

    let cwd = req
        .cwd
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let env_vars = crate::env_allowlist::minimal_child_env(req.env);

    let (program, args) = match kind.id.as_str() {
        "claude-desktop" => (
            "open".to_string(),
            vec!["-a".to_string(), "Claude Desktop".to_string()],
        ),
        "codex-cli" => ("codex".to_string(), vec![]),
        "cursor-agent" => ("cursor-agent".to_string(), vec![]),
        "kimi-cli" => ("kimi".to_string(), vec![]),
        "allternit-local" => ("allternit-local".to_string(), vec![]),
        "generic-mcp" => {
            let cmd = std::env::var("MCP_COMMAND").unwrap_or_default();
            let mut parts = cmd.split_whitespace().map(|s| s.to_string());
            let program = parts.next().unwrap_or_default();
            (program, parts.collect())
        }
        _ => {
            return Json(json!({
                "error": format!("Adapter {} cannot be spawned", kind.id),
            }));
        }
    };

    if program.is_empty() {
        return Json(json!({
            "error": format!("No command configured for adapter {}", kind.id),
        }));
    }

    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args)
        .current_dir(&cwd)
        .envs(&env_vars)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            // Detach the child so it keeps running after this request completes.
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            Json(json!(SpawnAdapterResponse {
                kind: kind.id.clone(),
                status: "spawned".to_string(),
                command: Some(format!("{} {}", program, args.join(" ")).trim().to_string()),
                pid,
            }))
        }
        Err(err) => Json(json!({
            "error": format!("Failed to spawn adapter {}: {}", kind.id, err),
        })),
    }
}

// ─── Plugin Install ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPluginRequest {
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub target: Option<String>,
    pub workspace_path: Option<String>,
    pub plugin: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPluginResponse {
    pub installed: bool,
    pub path: String,
    pub target: String,
    pub message: String,
}

fn normalize_plugin_id(id: &str) -> String {
    id.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-")
        .to_lowercase()
}

fn plugin_install_dir(
    plugin: &serde_json::Value,
    target: &str,
    workspace_path: Option<&str>,
) -> Option<PathBuf> {
    let base = normalize_plugin_id(plugin.get("id")?.as_str()?);
    let dir = match target {
        "claude-desktop" => dirs::home_dir()?.join(".claude").join("skills").join(&base),
        "codex-cli" => PathBuf::from(workspace_path.unwrap_or("."))
            .join("skills")
            .join(&base),
        "allternit-local" => PathBuf::from(workspace_path.unwrap_or("."))
            .join(".allternit")
            .join("plugins")
            .join(&base),
        "generic-mcp" => PathBuf::from(workspace_path.unwrap_or("."))
            .join("mcp-plugins")
            .join(&base),
        _ => PathBuf::from(workspace_path.unwrap_or("."))
            .join("skills")
            .join(&base),
    };
    Some(dir)
}

fn manifest_filename(target: &str) -> &'static str {
    match target {
        "claude-desktop" | "codex-cli" => "open-design.json",
        _ => "manifest.json",
    }
}

async fn install_plugin(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<InstallPluginRequest>,
) -> impl IntoResponse {
    let target = req.target.as_deref().unwrap_or("claude-desktop");
    let Some(dir) = plugin_install_dir(&req.plugin, target, req.workspace_path.as_deref()) else {
        return Json(json!({
            "error": "Could not resolve plugin install directory.",
        }));
    };

    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        return Json(json!({
            "error": format!("Failed to create plugin directory: {}", err),
        }));
    }

    let manifest_path = dir.join(manifest_filename(target));
    let manifest_body = match serde_json::to_string_pretty(&req.plugin) {
        Ok(body) => body,
        Err(err) => {
            return Json(json!({
                "error": format!("Failed to serialize plugin manifest: {}", err),
            }));
        }
    };

    if let Err(err) = tokio::fs::write(&manifest_path, manifest_body).await {
        return Json(json!({
            "error": format!("Failed to write plugin manifest: {}", err),
        }));
    }

    // Write a lightweight SKILL.md stub so the directory is recognized by skill scanners.
    let name = req
        .plugin
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Plugin");
    let skill_md = format!(
        "# {}\n\nAuto-installed Open Design plugin for agent {}.\n",
        name,
        req.agent_name.as_deref().unwrap_or(&req.agent_id)
    );
    let _ = tokio::fs::write(dir.join("SKILL.md"), skill_md).await;

    Json(json!(InstallPluginResponse {
        installed: true,
        path: manifest_path.to_string_lossy().to_string(),
        target: target.to_string(),
        message: format!("Installed {} to {}", name, dir.to_string_lossy()),
    }))
}

// ─── Design Import from URL ──────────────────────────────────────────────────
// POST /api/design/import-url
// Fetches a public web page server-side, extracts its CSS custom properties,
// color palette, and font families (plus up to 3 linked stylesheets), and
// returns a DesignSystem-shaped document the design-registry frontend can
// preview and save. The SSRF guard is load-bearing: literal IPs are rejected
// before DNS, every resolved address is classified, and each redirect hop is
// re-validated.

use std::net::IpAddr;

const IMPORT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 AllternitDesignImporter/1.0";
const IMPORT_MAX_REDIRECTS: usize = 3;
const IMPORT_MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const IMPORT_MAX_STYLESHEET_BYTES: usize = 1024 * 1024;
const IMPORT_MAX_STYLESHEETS: usize = 3;
const IMPORT_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const IMPORT_FALLBACK_COLORS: [&str; 4] = ["#111111", "#444444", "#888888", "#ffffff"];

#[derive(Debug, Clone, Deserialize)]
pub struct ImportUrlRequest {
    pub url: String,
}

enum ImportError {
    BadUrl(String),
    ForbiddenTarget(String),
    Fetch(String),
    NotHtml(String),
}

impl ImportError {
    fn message(&self) -> String {
        match self {
            ImportError::BadUrl(m) => m.clone(),
            ImportError::ForbiddenTarget(m) => m.clone(),
            ImportError::Fetch(m) => m.clone(),
            ImportError::NotHtml(m) => m.clone(),
        }
    }
}

// ─── SSRF guard ──────────────────────────────────────────────────────────────

/// Rejects the URL when its host is a literal IP (v4 or v6, including
/// IPv4-mapped v6) that is not publicly routable. Delegates to the shared
/// egress guard.
fn host_ip_literal_is_forbidden(host: &str) -> bool {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map(|ip| !allternit_factory_engine::egress::is_public_ip(ip))
        .unwrap_or(false)
}

/// Validates scheme, literal-IP host, and every DNS-resolved address.
async fn validate_target(url: &url::Url) -> Result<(), ImportError> {
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(ImportError::BadUrl(
            "Only http:// and https:// URLs are supported".to_string(),
        ));
    }
    let Some(host) = url.host_str() else {
        return Err(ImportError::BadUrl("URL has no host".to_string()));
    };
    if host_ip_literal_is_forbidden(host) {
        return Err(ImportError::ForbiddenTarget(
            "URL points to a local or private address, which is not allowed".to_string(),
        ));
    }
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ImportError::Fetch(format!("Could not resolve {}: {}", host, e)))?
        .collect();
    if addrs.is_empty() {
        return Err(ImportError::Fetch(format!("Could not resolve {}", host)));
    }
    if addrs.iter().any(|a| !allternit_factory_engine::egress::is_public_ip(a.ip())) {
        return Err(ImportError::ForbiddenTarget(
            "URL resolves to a local or private address, which is not allowed".to_string(),
        ));
    }
    Ok(())
}

struct FetchedResource {
    body: String,
    content_type: String,
    final_url: String,
}

/// Fetches a URL with a 10s timeout, no automatic redirects, a browser-ish
/// User-Agent, and a byte cap. Redirects (max 3) are followed manually so
/// every hop re-runs the SSRF validation.
async fn guarded_fetch(start: &url::Url, max_bytes: usize) -> Result<FetchedResource, ImportError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(IMPORT_FETCH_TIMEOUT)
        .user_agent(IMPORT_USER_AGENT)
        .build()
        .map_err(|e| ImportError::Fetch(format!("Could not build HTTP client: {}", e)))?;

    let mut current = start.clone();
    let mut hops = 0usize;
    loop {
        validate_target(&current).await?;

        let resp = client.get(current.as_str()).send().await.map_err(|e| {
            if e.is_timeout() {
                ImportError::Fetch("Timed out fetching the URL (10s limit)".to_string())
            } else {
                ImportError::Fetch(format!("Could not fetch URL: {}", e))
            }
        })?;

        let status = resp.status();
        if status.is_redirection() {
            if hops >= IMPORT_MAX_REDIRECTS {
                return Err(ImportError::Fetch("Too many redirects (max 3)".to_string()));
            }
            let Some(location) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                return Err(ImportError::Fetch(format!(
                    "Redirect (HTTP {}) without a Location header",
                    status
                )));
            };
            current = current
                .join(location)
                .map_err(|_| ImportError::Fetch("Invalid redirect Location header".to_string()))?;
            hops += 1;
            continue;
        }
        if !status.is_success() {
            return Err(ImportError::Fetch(format!("URL returned HTTP {}", status)));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        use futures::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|e| ImportError::Fetch(format!("Error reading response: {}", e)))?;
            bytes.extend_from_slice(&chunk);
            if bytes.len() > max_bytes {
                return Err(ImportError::Fetch(
                    "Response body is too large (2MB limit)".to_string(),
                ));
            }
        }

        return Ok(FetchedResource {
            body: String::from_utf8_lossy(&bytes).into_owned(),
            content_type,
            final_url: current.to_string(),
        });
    }
}

// ─── HTML / CSS extraction (pure functions) ──────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
struct HtmlExtraction {
    name: Option<String>,
    stylesheet_hrefs: Vec<String>,
    /// All CSS text found inline: `<style>` block contents plus `style="…"`
    /// attribute values. Linked stylesheets are appended to this at fetch time.
    css_text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct CssTokens {
    /// CSS custom properties (`--name: value`), deduped by name, first wins.
    vars: Vec<(String, String)>,
    /// Hex colors, deduped, ordered by frequency (ties keep first-seen order).
    colors: Vec<String>,
    /// Font families, deduped, first-seen order.
    fonts: Vec<String>,
}

/// Collapses whitespace, decodes a few common HTML entities, truncates.
fn clean_text(raw: &str, max_len: usize) -> String {
    let decoded = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let collapsed: String = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = collapsed.chars().take(max_len).collect();
    if collapsed.chars().count() > max_len {
        out.push('…');
    }
    out
}

fn extract_from_html(html: &str) -> HtmlExtraction {
    let mut out = HtmlExtraction::default();

    // <title> first, og:site_name as fallback.
    if let Ok(re) = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>") {
        if let Some(cap) = re.captures(html) {
            let title = clean_text(cap.get(1).map(|m| m.as_str()).unwrap_or(""), 80);
            if !title.is_empty() {
                out.name = Some(title);
            }
        }
    }
    if out.name.is_none() {
        if let Ok(meta_re) = regex::Regex::new(r"(?is)<meta\b[^>]*>") {
            if let Ok(prop_re) = regex::Regex::new(r#"(?is)property\s*=\s*["']og:site_name["']"#) {
                if let Ok(content_re) =
                    regex::Regex::new(r#"(?is)content\s*=\s*["']([^"']*)["']"#)
                {
                    'outer: for tag in meta_re.find_iter(html) {
                        if prop_re.find(tag.as_str()).is_none() {
                            continue;
                        }
                        if let Some(cap) = content_re.captures(tag.as_str()) {
                            let name = clean_text(cap.get(1).map(|m| m.as_str()).unwrap_or(""), 80);
                            if !name.is_empty() {
                                out.name = Some(name);
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }

    // Inline <style> blocks.
    if let Ok(style_re) = regex::Regex::new(r"(?is)<style[^>]*>(.*?)</style>") {
        for cap in style_re.captures_iter(html) {
            out.css_text.push_str(cap.get(1).map(|m| m.as_str()).unwrap_or(""));
            out.css_text.push('\n');
        }
    }
    // Inline style="…" attributes.
    if let Ok(attr_re) = regex::Regex::new(r#"(?is)style\s*=\s*["']([^"']*)["']"#) {
        for cap in attr_re.captures_iter(html) {
            out.css_text.push_str(cap.get(1).map(|m| m.as_str()).unwrap_or(""));
            out.css_text.push('\n');
        }
    }

    // <link rel="stylesheet" href="…"> (attribute order tolerant).
    if let Ok(link_re) = regex::Regex::new(r"(?is)<link\b[^>]*>") {
        if let Ok(rel_re) = regex::Regex::new(r#"(?is)rel\s*=\s*["'][^"']*stylesheet[^"']*["']"#) {
            if let Ok(href_re) = regex::Regex::new(r#"(?is)href\s*=\s*["']([^"']*)["']"#) {
                for tag in link_re.find_iter(html) {
                    if rel_re.find(tag.as_str()).is_none() {
                        continue;
                    }
                    if let Some(cap) = href_re.captures(tag.as_str()) {
                        if let Some(href) = cap.get(1).map(|m| m.as_str().trim().to_string()) {
                            if !href.is_empty() && !href.starts_with("data:") {
                                out.stylesheet_hrefs.push(href);
                            }
                        }
                    }
                }
            }
        }
    }

    out
}

/// Extracts CSS custom properties, hex colors, and font families from a CSS
/// text blob (page inline CSS or a fetched stylesheet).
fn extract_css_tokens(css: &str) -> CssTokens {
    let mut out = CssTokens::default();

    if let Ok(var_re) = regex::Regex::new(r"(?m)--([a-zA-Z0-9_-]+)\s*:\s*([^;}{]+)") {
        for cap in var_re.captures_iter(css) {
            let name = cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
            let value = cap
                .get(2)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            if name.is_empty() || value.is_empty() {
                continue;
            }
            if out.vars.iter().any(|(n, _)| *n == name) {
                continue;
            }
            out.vars.push((name, value));
        }
    }

    if let Ok(hex_re) = regex::Regex::new(r"#([0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b") {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for cap in hex_re.captures_iter(css) {
            let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let normalized = format!(
                "#{}",
                if raw.len() == 3 {
                    let mut s = String::with_capacity(6);
                    for c in raw.chars() {
                        s.push(c);
                        s.push(c);
                    }
                    s
                } else {
                    raw.to_string()
                }
                .to_lowercase()
            );
            if !seen.contains_key(&normalized) {
                order.push(normalized.clone());
            }
            *seen.entry(normalized).or_insert(0) += 1;
        }
        // Stable sort keeps first-seen order among equal frequencies.
        let mut ranked = order;
        ranked.sort_by(|a, b| seen[b].cmp(&seen[a]));
        out.colors = ranked;
    }

    if let Ok(font_re) = regex::Regex::new(r"(?i)font-family\s*:\s*([^;}{]+)") {
        for cap in font_re.captures_iter(css) {
            let decl = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            for part in decl.split(',') {
                let family = part
                    .trim()
                    .trim_matches(|c| c == '\'' || c == '"')
                    .trim()
                    .to_string();
                if family.is_empty()
                    || family.starts_with("var(")
                    || out.fonts.iter().any(|f| f == &family)
                {
                    continue;
                }
                out.fonts.push(family);
            }
        }
    }

    out
}

// ─── design.md assembly ──────────────────────────────────────────────────────

fn build_design_md(
    name: &str,
    source_url: &str,
    tokens: &CssTokens,
) -> String {
    let mut md = String::new();
    md.push_str(&format!("# Design System: {}\n\n", name));
    md.push_str(&format!("Imported from {}\n\n", source_url));

    md.push_str("## Colors\n\n");
    if tokens.colors.is_empty() {
        md.push_str("_No hex colors were detected._\n");
    } else {
        for color in tokens.colors.iter().take(12) {
            md.push_str(&format!("- `{}`\n", color));
        }
    }
    md.push('\n');

    md.push_str("## Typography\n\n");
    if tokens.fonts.is_empty() {
        md.push_str("_No font-family declarations were detected._\n");
    } else {
        for font in tokens.fonts.iter().take(10) {
            md.push_str(&format!("- {}\n", font));
        }
    }
    md.push('\n');

    md.push_str("## Tokens\n\n");
    if tokens.vars.is_empty() {
        md.push_str("No CSS custom properties were found on this page.\n");
    } else {
        for (var, value) in tokens.vars.iter().take(60) {
            md.push_str(&format!("- `--{}`: {}\n", var, value));
        }
    }

    md
}

// ─── Route handler ───────────────────────────────────────────────────────────

async fn import_url(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ImportUrlRequest>,
) -> impl IntoResponse {
    match import_url_inner(&req.url).await {
        Ok(design) => Json(json!({ "ok": true, "design": design })),
        Err(err) => Json(json!({ "ok": false, "error": err.message() })),
    }
}

async fn import_url_inner(url_str: &str) -> Result<serde_json::Value, ImportError> {
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return Err(ImportError::BadUrl("A URL is required".to_string()));
    }
    let start = url::Url::parse(trimmed).map_err(|_| {
        ImportError::BadUrl(format!("Invalid URL: {}", trimmed))
    })?;

    let page = guarded_fetch(&start, IMPORT_MAX_PAGE_BYTES).await?;
    let content_type = page.content_type.clone();
    if !content_type.is_empty()
        && !content_type.contains("text/html")
        && !content_type.contains("application/xhtml")
    {
        return Err(ImportError::NotHtml(format!(
            "not an HTML page (content-type: {})",
            content_type
        )));
    }

    let base = url::Url::parse(&page.final_url).unwrap_or(start);
    let mut extracted = extract_from_html(&page.body);

    // Follow up to 3 linked stylesheets through the same guarded fetcher.
    let hrefs = extracted.stylesheet_hrefs.clone();
    let mut fetched_sheets = 0usize;
    for href in &hrefs {
        if fetched_sheets >= IMPORT_MAX_STYLESHEETS {
            break;
        }
        let Ok(abs) = base.join(href) else {
            continue;
        };
        if abs.scheme() != "http" && abs.scheme() != "https" {
            continue;
        }
        match guarded_fetch(&abs, IMPORT_MAX_STYLESHEET_BYTES).await {
            Ok(sheet) => {
                extracted.css_text.push_str(&sheet.body);
                extracted.css_text.push('\n');
                fetched_sheets += 1;
            }
            Err(_) => continue,
        }
    }

    let tokens = extract_css_tokens(&extracted.css_text);

    let host = base.host_str().unwrap_or("imported-site").to_string();
    let name = extracted.name.clone().unwrap_or(host);
    let preview_colors: Vec<String> = if tokens.colors.is_empty() {
        IMPORT_FALLBACK_COLORS.iter().map(|s| s.to_string()).collect()
    } else {
        tokens.colors.iter().take(4).cloned().collect()
    };
    let description = format!(
        "Imported from {} — {} colors, {} font families, {} CSS custom properties extracted.",
        base,
        tokens.colors.len(),
        tokens.fonts.len(),
        tokens.vars.len()
    );
    let design_md = build_design_md(&name, base.as_str(), &tokens);

    Ok(json!({
        "name": name,
        "description": description,
        "vibe": "Imported",
        "author": "imported",
        "tags": ["imported", "web"],
        "designMd": design_md,
        "previewColors": preview_colors,
    }))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod import_url_tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    const FIXTURE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
  <title>Acme &amp; Co — Home</title>
  <meta property="og:site_name" content="Acme OG">
  <link rel="stylesheet" href="/assets/app.css">
  <link href="https://cdn.example.com/lib.css" rel="stylesheet">
  <link rel="icon" href="/favicon.ico">
  <style>
    :root {
      --color-primary: #FF6600;
      --color-secondary: #3366ff;
      --radius-base: 8px;
    }
    body { font-family: 'Inter', sans-serif; color: #f60; background: #ffffff; }
    h1 { font-family: "Space Grotesk", 'Inter', sans-serif; color: #3366FF; }
    .card { color: #fff; }
  </style>
</head>
<body style="margin: 0; font-family: Georgia, serif;">
  <h1 style="color: #FF6600;">Hello</h1>
</body>
</html>"#;

    #[test]
    fn extract_from_html_pulls_title_hrefs_and_inline_css() {
        let ex = extract_from_html(FIXTURE_HTML);

        // <title> wins over og:site_name, entities decoded, whitespace collapsed.
        assert_eq!(ex.name.as_deref(), Some("Acme & Co — Home"));

        // Both stylesheet links found (either attribute order), icon skipped.
        assert_eq!(
            ex.stylesheet_hrefs,
            vec!["/assets/app.css", "https://cdn.example.com/lib.css"]
        );

        // Inline CSS (style block + style attributes) is collected.
        assert!(ex.css_text.contains("--color-primary"));
        assert!(ex.css_text.contains("margin: 0"));
    }

    #[test]
    fn extract_from_html_falls_back_to_og_site_name() {
        let html = r#"<html><head>
          <meta property="og:site_name" content="OG Name Here">
        </head><body></body></html>"#;
        let ex = extract_from_html(html);
        assert_eq!(ex.name.as_deref(), Some("OG Name Here"));
        assert!(ex.css_text.is_empty());
    }

    #[test]
    fn extract_css_tokens_orders_colors_by_frequency() {
        let tokens = extract_css_tokens(&extract_from_html(FIXTURE_HTML).css_text);

        // #ff6600 appears 3x (var + body + h1), #3366ff 2x, #ffffff/#fff 1x each.
        assert_eq!(tokens.colors.first().map(String::as_str), Some("#ff6600"));
        let pos_3366ff = tokens.colors.iter().position(|c| c == "#3366ff").unwrap();
        let pos_ffffff = tokens.colors.iter().position(|c| c == "#ffffff").unwrap();
        let pos_fff = tokens.colors.iter().position(|c| c == "#ffffff").unwrap();
        assert!(pos_3366ff < pos_ffffff);
        assert_eq!(pos_ffffff, pos_fff); // #fff normalized into #ffffff, deduped
        assert_eq!(tokens.colors.iter().filter(|c| *c == "#ffffff").count(), 1);

        // Custom properties deduped, first value wins.
        assert!(tokens
            .vars
            .contains(&("color-primary".to_string(), "#FF6600".to_string())));
        assert!(tokens
            .vars
            .contains(&("radius-base".to_string(), "8px".to_string())));

        // Font families in first-seen order, quotes stripped, fallbacks kept.
        assert_eq!(tokens.fonts.first().map(String::as_str), Some("Inter"));
        assert!(tokens.fonts.contains(&"Space Grotesk".to_string()));
        assert!(tokens.fonts.contains(&"Georgia".to_string()));
    }

    #[test]
    fn extract_css_tokens_handles_empty_input() {
        let tokens = extract_css_tokens("");
        assert!(tokens.vars.is_empty());
        assert!(tokens.colors.is_empty());
        assert!(tokens.fonts.is_empty());
    }

    #[test]
    fn is_forbidden_ip_blocks_local_and_private_addresses() {
        let forbidden: Vec<IpAddr> = vec![
            Ipv4Addr::new(127, 0, 0, 1).into(),
            Ipv4Addr::new(127, 8, 9, 10).into(),
            Ipv4Addr::new(10, 1, 2, 3).into(),
            Ipv4Addr::new(172, 16, 5, 4).into(),
            Ipv4Addr::new(172, 31, 255, 255).into(),
            Ipv4Addr::new(192, 168, 1, 1).into(),
            Ipv4Addr::new(169, 254, 1, 1).into(),
            Ipv4Addr::new(0, 0, 0, 0).into(),
            Ipv4Addr::new(100, 64, 0, 1).into(), // shared CGNAT — blocked too
            Ipv6Addr::LOCALHOST.into(),
            "fd00::1".parse::<IpAddr>().unwrap(),
            "fe80::1".parse::<IpAddr>().unwrap(),
            "::".parse::<IpAddr>().unwrap(),
            // IPv4-mapped loopback/private must not slip through as v6.
            "::ffff:127.0.0.1".parse::<IpAddr>().unwrap(),
            "::ffff:10.0.0.1".parse::<IpAddr>().unwrap(),
        ];
        for ip in forbidden {
            assert!(!allternit_factory_engine::egress::is_public_ip(ip), "{} should be forbidden", ip);
        }

        let allowed: Vec<IpAddr> = vec![
            Ipv4Addr::new(8, 8, 8, 8).into(),
            Ipv4Addr::new(1, 1, 1, 1).into(),
            Ipv4Addr::new(93, 184, 216, 34).into(), // example.com
            "2606:4700:4700::1111".parse::<IpAddr>().unwrap(),
        ];
        for ip in allowed {
            assert!(allternit_factory_engine::egress::is_public_ip(ip), "{} should be allowed", ip);
        }
    }

    #[test]
    fn host_ip_literal_is_forbidden_classifies_url_hosts() {
        assert!(host_ip_literal_is_forbidden("127.0.0.1"));
        assert!(host_ip_literal_is_forbidden("0.0.0.0"));
        assert!(host_ip_literal_is_forbidden("::1"));
        assert!(host_ip_literal_is_forbidden("192.168.0.44"));
        assert!(!host_ip_literal_is_forbidden("example.com"));
        assert!(!host_ip_literal_is_forbidden("1.1.1.1"));
    }

    #[test]
    fn build_design_md_notes_when_no_tokens_found() {
        let tokens = CssTokens::default();
        let md = build_design_md("Bare Site", "https://bare.example/", &tokens);
        assert!(md.starts_with("# Design System: Bare Site"));
        assert!(md.contains("## Colors"));
        assert!(md.contains("## Typography"));
        assert!(md.contains("## Tokens"));
        assert!(md.contains("No CSS custom properties were found on this page."));
    }

    #[test]
    fn build_design_md_stays_compact_with_many_tokens() {
        let mut tokens = CssTokens::default();
        for i in 0..200 {
            tokens.vars.push((format!("var-{}", i), format!("{}px", i)));
            tokens.colors.push(format!("#{:06x}", i));
            tokens.fonts.push(format!("Font{}", i));
        }
        let md = build_design_md("Big Site", "https://big.example/", &tokens);
        let lines = md.lines().count();
        assert!(
            lines <= 120,
            "designMd should stay under 120 lines, got {}",
            lines
        );
        assert!(md.contains("--var-0"));
        assert!(!md.contains("--var-60"));
    }
}
