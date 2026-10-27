//! The one list of coding CLIs Allternit supports, plus the ones a user adds.
//!
//! - **Catalog**: `cmd/gizzi-code/src/runtime/providers/discovery/cli-catalog.json`
//!   is the single source of truth. gizzi's subprocess discovery builds its
//!   brains from it, and this runtime builds the CLI provider rows, sign-in
//!   metadata and catalog endpoint from the same file (embedded below). A tool
//!   the Factory harness installs points at its `harness.json` key, so the
//!   version pin and licence live in one place; other tools carry their own
//!   pinned install recipe here.
//! - **Custom tools** (`POST /api/v1/cli-tools/custom`): stored per user in
//!   `custom_cli_tools` and mirrored to `<harness home>/custom-clis.json`,
//!   which gizzi's subprocess discovery reads like the catalog, so a custom
//!   tool runs as a brain through the same channel. Every command is argv —
//!   a bare program name or absolute path plus an array of arguments; the
//!   prompt is exactly one argv element; nothing goes through a shell.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::get_user;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cli-tools/catalog", get(get_catalog))
        .route("/cli-tools/custom", post(create_custom))
        .route("/cli-tools/custom/:custom_id", delete(delete_custom))
}

// ─── Catalog ─────────────────────────────────────────────────────────────────

const CATALOG_JSON: &str =
    include_str!("../../gizzi-code/src/runtime/providers/discovery/cli-catalog.json");

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CatalogModel {
    pub id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Headless {
    pub args: Vec<String>,
    #[serde(default)]
    pub model_args: Vec<String>,
}

/// Where a CLI keeps its sign-in: any listed file under `$HOME` or env var.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct AuthCheck {
    #[serde(default)]
    pub files: Vec<Vec<String>>,
    #[serde(default)]
    pub env: Vec<String>,
}

/// `{harness}` = the Factory installs it (pin + licence in harness.json);
/// otherwise `{method: npm|uv|brew|script, package|url, pinnedVersion}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallRecipe {
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub pinned_version: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// brew: a cask, not a formula.
    #[serde(default)]
    pub cask: bool,
    /// Extra installer flags before the package (`--python 3.12` for uv).
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliTool {
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub name: String,
    pub vendor: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    pub bin: String,
    /// `builtin` (gizzi has a protocol adapter), `text` (one-shot argv from
    /// `headless`), or `unsupported` (listed, not runnable as a brain yet).
    pub adapter: String,
    #[serde(default)]
    pub headless: Option<Headless>,
    #[serde(default)]
    pub models: Vec<CatalogModel>,
    /// `subscription` | `api_key` | `none`.
    pub sign_in: String,
    #[serde(default)]
    pub login: Vec<String>,
    #[serde(default)]
    pub sign_in_page: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthCheck>,
    /// Env var that points the CLI at its own config/sign-in dir, which is how
    /// a second account gets its own sign-in (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`).
    /// Absent = the CLI stays single-account.
    #[serde(default)]
    pub account_home_env: Option<String>,
    #[serde(default)]
    pub install: Option<InstallRecipe>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub terms_url: Option<String>,
}

impl CliTool {
    /// The model gizzi registers first for this CLI ("default" when it lists none).
    pub fn default_model(&self) -> &str {
        self.models
            .first()
            .map(|m| m.id.as_str())
            .unwrap_or("default")
    }

    pub fn model_ids(&self) -> Vec<&str> {
        self.models.iter().map(|m| m.id.as_str()).collect()
    }
}

#[derive(Deserialize)]
struct Catalog {
    tools: Vec<CliTool>,
}

fn parse_catalog(text: &str) -> Vec<CliTool> {
    serde_json::from_str::<Catalog>(text)
        .map(|c| c.tools)
        .unwrap_or_default()
}

static CATALOG: Lazy<Vec<CliTool>> = Lazy::new(|| parse_catalog(CATALOG_JSON));

/// Every supported CLI, in catalog order.
pub(crate) fn tools() -> &'static [CliTool] {
    &CATALOG
}

/// A catalog CLI by provider id or alias (`claude` → `claude-cli`).
pub(crate) fn find(id: &str) -> Option<&'static CliTool> {
    CATALOG
        .iter()
        .find(|t| t.id == id || t.aliases.iter().any(|a| a == id))
}

pub(crate) fn license_requires_terms(license: &str) -> bool {
    !matches!(license, "apache" | "mit" | "bsd")
}

/// How this runtime installs a catalog CLI the Factory harness doesn't.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DirectInstall {
    /// argv run as-is (npm into the harness prefix, uv/brew).
    Argv {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// `curl -fsSL <url>` piped into `bash` (two processes, no shell string).
    Script { url: String },
}

pub(crate) fn direct_install(tool: &CliTool) -> Option<DirectInstall> {
    let recipe = tool.install.as_ref()?;
    if recipe.harness.is_some() {
        return None;
    }
    let root = allternit_factory_engine::registry::harness_root();
    let bin = allternit_factory_engine::registry::harness_bin_dir();
    match recipe.method.as_deref()? {
        "npm" => {
            let package = recipe.package.as_ref()?;
            let spec = match &recipe.pinned_version {
                Some(pin) => format!("{package}@{pin}"),
                None => package.clone(),
            };
            let mut args: Vec<String> = vec![
                "install".into(),
                "-g".into(),
                "--prefix".into(),
                root.display().to_string(),
                "--no-audit".into(),
                "--no-fund".into(),
            ];
            args.extend(recipe.extra_args.iter().cloned());
            args.push(spec);
            Some(DirectInstall::Argv {
                program: "npm".into(),
                args,
                env: Vec::new(),
            })
        }
        "uv" => {
            let package = recipe.package.as_ref()?;
            let spec = match &recipe.pinned_version {
                Some(pin) => format!("{package}=={pin}"),
                None => package.clone(),
            };
            let mut args: Vec<String> = vec!["tool".into(), "install".into(), "--force".into()];
            args.extend(recipe.extra_args.iter().cloned());
            args.push(spec);
            Some(DirectInstall::Argv {
                program: "uv".into(),
                args,
                env: vec![("UV_TOOL_BIN_DIR".into(), bin.display().to_string())],
            })
        }
        "brew" => {
            let mut args: Vec<String> = vec!["install".into()];
            if recipe.cask {
                args.push("--cask".into());
            }
            args.extend(recipe.extra_args.iter().cloned());
            args.push(recipe.package.clone()?);
            Some(DirectInstall::Argv {
                program: "brew".into(),
                args,
                env: Vec::new(),
            })
        }
        "script" => recipe
            .url
            .as_ref()
            .filter(|u| u.starts_with("https://"))
            .map(|url| DirectInstall::Script { url: url.clone() }),
        _ => None,
    }
}

fn direct_install_available(plan: &DirectInstall) -> bool {
    let on_path = |p: &str| crate::cli_tools::resolve_binary(p).is_some();
    match plan {
        DirectInstall::Argv { program, .. } => on_path(program),
        DirectInstall::Script { .. } => !cfg!(windows) && on_path("curl") && on_path("bash"),
    }
}

/// The install offer for a catalog CLI the harness doesn't install.
pub(crate) fn direct_install_json(tool: &CliTool) -> Value {
    let Some(plan) = direct_install(tool) else {
        return Value::Null;
    };
    let license = tool.license.clone().unwrap_or_else(|| "undeclared".into());
    json!({
        "available": direct_install_available(&plan),
        "requiresTerms": license_requires_terms(&license),
        "license": license,
        "termsUrl": tool.terms_url,
        "pin": tool.install.as_ref().and_then(|i| i.pinned_version.clone()),
    })
}

// ─── Custom tools ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomTool {
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    pub name: String,
    pub command: String,
    pub prompt_args: Vec<String>,
    #[serde(default)]
    pub model_args: Vec<String>,
    #[serde(default)]
    pub version_args: Vec<String>,
    #[serde(default)]
    pub auth_check_args: Vec<String>,
    #[serde(default)]
    pub install_command: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomToolBody {
    pub name: String,
    pub command: String,
    pub prompt_args: Vec<String>,
    #[serde(default)]
    pub model_args: Option<Vec<String>>,
    #[serde(default)]
    pub version_args: Option<Vec<String>>,
    #[serde(default)]
    pub auth_check_args: Option<Vec<String>>,
    #[serde(default)]
    pub install_command: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
}

/// Characters a shell would treat specially. None of them may appear in a
/// custom command or install command (they never reach a shell, but a value
/// that needs them is not a plain program invocation).
const SHELL_META: &[char] = &[
    ';', '&', '|', '$', '`', '<', '>', '(', ')', '{', '}', '[', ']', '*', '?', '!', '~', '\'', '"',
    '\\', '#', '\n', '\r', '\t', '\0',
];

/// Package managers a custom tool's install command may run.
const INSTALL_PROGRAMS: &[&str] = &[
    "npm", "pnpm", "bun", "yarn", "brew", "pip", "pip3", "pipx", "uv", "cargo", "go", "gem",
];

fn valid_program(command: &str) -> bool {
    if command.is_empty()
        || command.len() > 512
        || command.contains(SHELL_META)
        || command.starts_with('-')
    {
        return false;
    }
    let absolute = std::path::Path::new(command).is_absolute();
    if absolute {
        // An absolute path: no parent-directory hops, no trailing slash.
        !command.split(['/', '\\']).any(|part| part == "..") && !command.ends_with('/')
    } else {
        command
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    }
}

fn valid_arg_list(args: &[String], max: usize) -> bool {
    args.len() <= max
        && args
            .iter()
            .all(|a| a.len() <= 512 && !a.contains(['\0', '\n', '\r']))
}

/// The argv of a custom tool's install command (`npm install -g foo@1.2`),
/// or why it can't run.
pub(crate) fn parse_install_command(cmd: &str) -> Result<Vec<String>, &'static str> {
    let cmd = cmd.trim();
    if cmd.is_empty() || cmd.len() > 512 {
        return Err("installCommand must be 1–512 characters");
    }
    if cmd.contains(SHELL_META) {
        return Err(
            "installCommand must be one plain command — no shell operators, quotes or variables",
        );
    }
    let argv: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
    if !INSTALL_PROGRAMS.contains(&argv[0].as_str()) {
        return Err("installCommand must run a package manager (npm, pnpm, bun, yarn, brew, pip, pip3, pipx, uv, cargo, go, gem)");
    }
    Ok(argv)
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    out.chars()
        .take(32)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Check a request body and build the tool to store.
pub(crate) fn validate_custom(body: CustomToolBody, user_id: &str) -> Result<CustomTool, String> {
    let name = body.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("name must be 1–60 characters".into());
    }
    let command = body.command.trim().to_string();
    if !valid_program(&command) {
        return Err("command must be a program name (like `mytool`) or an absolute path, with no shell characters".into());
    }
    if body.prompt_args.is_empty() || !valid_arg_list(&body.prompt_args, 32) {
        return Err("promptArgs must be 1–32 arguments".into());
    }
    let prompt_slots = body
        .prompt_args
        .iter()
        .filter(|a| a.as_str() == "{prompt}")
        .count();
    let prompt_mentions = body
        .prompt_args
        .iter()
        .filter(|a| a.contains("{prompt}"))
        .count();
    if prompt_slots != 1 || prompt_mentions != 1 {
        return Err(
            "promptArgs must contain exactly one argument that is exactly \"{prompt}\"".into(),
        );
    }
    let model_args = body.model_args.unwrap_or_default();
    if !valid_arg_list(&model_args, 8)
        || model_args.iter().any(|a| a.contains("{prompt}"))
        || (!model_args.is_empty() && !model_args.iter().any(|a| a.contains("{model}")))
    {
        return Err(
            "modelArgs must be up to 8 arguments containing \"{model}\" (and not \"{prompt}\")"
                .into(),
        );
    }
    let version_args = body.version_args.unwrap_or_default();
    let auth_check_args = body.auth_check_args.unwrap_or_default();
    for (label, list) in [
        ("versionArgs", &version_args),
        ("authCheckArgs", &auth_check_args),
    ] {
        if !valid_arg_list(list, 8)
            || list
                .iter()
                .any(|a| a.contains("{prompt}") || a.contains("{model}"))
        {
            return Err(format!("{label} must be up to 8 plain arguments"));
        }
    }
    let install_command = body
        .install_command
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    if let Some(cmd) = &install_command {
        parse_install_command(cmd).map_err(str::to_string)?;
    }
    let homepage = body
        .homepage
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty());
    if let Some(url) = &homepage {
        if !url.starts_with("https://") || url.len() > 300 || url.contains(char::is_whitespace) {
            return Err("homepage must be an https:// link".into());
        }
    }
    let base = match slug(&name) {
        s if s.is_empty() => "tool".to_string(),
        s => s,
    };
    let suffix: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(6)
        .collect();
    Ok(CustomTool {
        id: format!("custom-{base}-{suffix}"),
        user_id: user_id.to_string(),
        name,
        command,
        prompt_args: body.prompt_args,
        model_args,
        version_args,
        auth_check_args,
        install_command,
        homepage,
    })
}

fn json_list(list: &[String]) -> Option<String> {
    (!list.is_empty()).then(|| serde_json::to_string(list).unwrap_or_default())
}

fn parse_list(text: Option<String>) -> Vec<String> {
    text.and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn row_to_custom(row: &rusqlite::Row) -> rusqlite::Result<CustomTool> {
    Ok(CustomTool {
        id: row.get(0)?,
        user_id: row.get(1)?,
        name: row.get(2)?,
        command: row.get(3)?,
        prompt_args: parse_list(row.get(4)?),
        model_args: parse_list(row.get(5)?),
        version_args: parse_list(row.get(6)?),
        auth_check_args: parse_list(row.get(7)?),
        install_command: row.get(8)?,
        homepage: row.get(9)?,
    })
}

const CUSTOM_COLUMNS: &str =
    "id, user_id, name, command, prompt_args, model_args, version_args, auth_check_args, install_command, homepage";

pub(crate) fn insert_custom(conn: &Connection, tool: &CustomTool) -> rusqlite::Result<()> {
    conn.execute(
        &format!("INSERT INTO custom_cli_tools ({CUSTOM_COLUMNS}, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
        params![
            tool.id,
            tool.user_id,
            tool.name,
            tool.command,
            serde_json::to_string(&tool.prompt_args).unwrap_or_default(),
            json_list(&tool.model_args),
            json_list(&tool.version_args),
            json_list(&tool.auth_check_args),
            tool.install_command,
            tool.homepage,
            chrono::Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// The user's custom tools, oldest first; `None` = every user's (the mirror).
pub(crate) fn list_custom(
    conn: &Connection,
    user_id: Option<&str>,
) -> rusqlite::Result<Vec<CustomTool>> {
    let sql = match user_id {
        Some(_) => format!("SELECT {CUSTOM_COLUMNS} FROM custom_cli_tools WHERE user_id = ?1 ORDER BY created_at, id"),
        None => format!("SELECT {CUSTOM_COLUMNS} FROM custom_cli_tools ORDER BY created_at, id"),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = match user_id {
        Some(user) => stmt
            .query_map(params![user], row_to_custom)?
            .collect::<Result<Vec<_>, _>>()?,
        None => stmt
            .query_map([], row_to_custom)?
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(rows)
}

pub(crate) fn get_custom(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> rusqlite::Result<Option<CustomTool>> {
    conn.query_row(
        &format!("SELECT {CUSTOM_COLUMNS} FROM custom_cli_tools WHERE user_id = ?1 AND id = ?2"),
        params![user_id, id],
        row_to_custom,
    )
    .optional()
}

pub(crate) fn delete_custom_row(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "DELETE FROM custom_cli_tools WHERE user_id = ?1 AND id = ?2",
        params![user_id, id],
    )? > 0)
}

/// `<harness home>/custom-clis.json`, which gizzi's subprocess discovery reads
/// (`customCliFile()` in subprocess.ts).
pub(crate) fn mirror_path() -> PathBuf {
    allternit_factory_engine::registry::harness_root().join("custom-clis.json")
}

/// The mirror's JSON: catalog-shaped entries gizzi turns into brains.
pub(crate) fn mirror_json(tools: &[CustomTool]) -> Value {
    json!({
        "version": 1,
        "tools": tools.iter().map(|t| json!({
            "id": t.id,
            "userId": t.user_id,
            "name": t.name,
            "vendor": "Custom",
            "bin": t.command,
            "adapter": "text",
            "custom": true,
            "headless": {"args": t.prompt_args, "modelArgs": t.model_args},
            "verifyArgs": t.version_args,
            "signIn": "none",
        })).collect::<Vec<_>>(),
    })
}

/// Rewrite the mirror from the store (every user's tools on this runtime).
pub(crate) fn write_mirror(conn: &Connection) -> std::io::Result<()> {
    let tools = list_custom(conn, None).map_err(std::io::Error::other)?;
    let path = mirror_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&mirror_json(&tools)).unwrap_or_default(),
    )?;
    std::fs::rename(&tmp, &path)
}

/// A custom tool from the mirror (for status rows that have no DB handle).
pub(crate) fn mirrored_custom(user_id: Option<&str>) -> Vec<CustomTool> {
    let Ok(text) = std::fs::read_to_string(mirror_path()) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let list = |v: Option<&Value>| -> Vec<String> {
                v.and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            Some(CustomTool {
                id: t.get("id")?.as_str()?.to_string(),
                user_id: t
                    .get("userId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: t.get("name")?.as_str()?.to_string(),
                command: t.get("bin")?.as_str()?.to_string(),
                prompt_args: list(t.get("headless").and_then(|h| h.get("args"))),
                model_args: list(t.get("headless").and_then(|h| h.get("modelArgs"))),
                version_args: list(t.get("verifyArgs")),
                auth_check_args: Vec::new(),
                install_command: None,
                homepage: None,
            })
        })
        .filter(|t| user_id.map_or(true, |u| t.user_id == u))
        .collect()
}

/// A custom tool's executable, if present on this computer.
pub(crate) fn resolve_custom(tool: &CustomTool) -> Option<PathBuf> {
    let path = std::path::Path::new(&tool.command);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    crate::cli_tools::resolve_binary(&tool.command)
}

/// Signed in, by the tool's own check (exit 0); without one, an installed
/// custom tool is taken as ready — the user brought it configured.
pub(crate) fn custom_authenticated(tool: &CustomTool, path: &std::path::Path) -> bool {
    if tool.auth_check_args.is_empty() {
        return true;
    }
    run_quiet(path, &tool.auth_check_args)
}

fn run_quiet(path: &std::path::Path, args: &[String]) -> bool {
    let child = std::process::Command::new(path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return false };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

pub(crate) fn custom_install_json(tool: &CustomTool) -> Value {
    let Some(cmd) = &tool.install_command else {
        return Value::Null;
    };
    let available = parse_install_command(cmd)
        .ok()
        .is_some_and(|argv| crate::cli_tools::resolve_binary(&argv[0]).is_some());
    json!({
        "available": available,
        "requiresTerms": true,
        "license": "custom",
        "termsUrl": tool.homepage,
        "pin": Value::Null,
    })
}

// ─── Endpoint rows ───────────────────────────────────────────────────────────

fn catalog_row(tool: &CliTool, installed: bool, authenticated: bool) -> Value {
    let status = match (installed, authenticated) {
        (false, _) => "not_installed",
        (true, true) => "ok",
        (true, false) => "missing",
    };
    let install = crate::cli_tools::install_json(&tool.id, &tool.bin);
    json!({
        "id": tool.id,
        "name": tool.name,
        "vendor": tool.vendor,
        "homepage": tool.homepage,
        "logo": tool.logo,
        "installed": installed,
        "authenticated": installed && authenticated,
        "status": status,
        "install": install,
        "signIn": tool.sign_in,
        "custom": false,
        "runnable": tool.adapter != "unsupported",
    })
}

pub(crate) fn custom_row(tool: &CustomTool) -> Value {
    let path = resolve_custom(tool);
    let installed = path.is_some();
    let authenticated = path
        .as_deref()
        .is_some_and(|p| custom_authenticated(tool, p));
    let status = match (installed, authenticated, tool.auth_check_args.is_empty()) {
        (false, ..) => "not_installed",
        (true, true, true) => "unknown",
        (true, true, false) => "ok",
        (true, false, _) => "missing",
    };
    json!({
        "id": tool.id,
        "name": tool.name,
        "vendor": "Custom",
        "homepage": tool.homepage,
        "logo": Value::Null,
        "installed": installed,
        "authenticated": authenticated,
        "status": status,
        "install": custom_install_json(tool),
        "signIn": "none",
        "custom": true,
        "runnable": true,
        "command": tool.command,
        "promptArgs": tool.prompt_args,
        "modelArgs": tool.model_args,
    })
}

fn json_error(status: StatusCode, error: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({"error": error, "message": message.into()})),
    )
        .into_response()
}

/// Custom tools run an arbitrary program on this machine, so they are offered
/// only where the runtime belongs to one person: the Desktop sidecar (it has a
/// desktop access token) or a self-hosted install. A shared hosted API never
/// accepts or runs them.
pub(crate) fn custom_tools_allowed(state: &AppState) -> bool {
    state.config.desktop_access_token().is_some() || state.config.self_hosted()
}

async fn get_catalog(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(user) = get_user(&headers) else {
        return json_error(StatusCode::UNAUTHORIZED, "Unauthorized", "Sign in first");
    };
    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;
    let custom_allowed = custom_tools_allowed(&state);
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let rows = crate::provider_routes::cli_status_by_id(&connected);
        let mut tools: Vec<Value> = tools()
            .iter()
            .map(|tool| {
                let (installed, authed) = rows
                    .get(tool.id.as_str())
                    .copied()
                    .unwrap_or((false, false));
                catalog_row(tool, installed, authed)
            })
            .collect();
        if custom_allowed {
            let conn = db.connect().map_err(|e| e.to_string())?;
            let custom = list_custom(&conn, Some(&user.user_id)).map_err(|e| e.to_string())?;
            // Keep gizzi's copy current (a lost or stale mirror heals here).
            if let Err(error) = write_mirror(&conn) {
                tracing::warn!(%error, "could not write the custom CLI mirror");
            }
            tools.extend(custom.iter().map(custom_row));
        }
        Ok(tools)
    })
    .await;
    match result {
        Ok(Ok(tools)) => Json(json!({ "tools": tools, "customAllowed": custom_allowed })).into_response(),
        Ok(Err(error)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", error),
        Err(error) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            error.to_string(),
        ),
    }
}

async fn create_custom(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<CustomToolBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(user) = get_user(&headers) else {
        return json_error(StatusCode::UNAUTHORIZED, "Unauthorized", "Sign in first");
    };
    if !custom_tools_allowed(&state) {
        return json_error(
            StatusCode::FORBIDDEN,
            "custom_tools_disabled",
            "Custom coding tools can only be added on your own computer.",
        );
    }
    let Ok(Json(body)) = body else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "Expected {name, command, promptArgs[]}",
        );
    };
    let tool = match validate_custom(body, &user.user_id) {
        Ok(tool) => tool,
        Err(message) => return json_error(StatusCode::BAD_REQUEST, "invalid_tool", message),
    };
    // A custom tool can't take a catalog CLI's place.
    if find(&tool.command).is_some() || tools().iter().any(|t| t.bin == tool.command) {
        return json_error(
            StatusCode::CONFLICT,
            "already_supported",
            format!(
                "{} is already in the list — use it from there",
                tool.command
            ),
        );
    }
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let existing = list_custom(&conn, Some(&tool.user_id)).map_err(|e| e.to_string())?;
        if existing.len() >= 50 {
            return Err("limit".into());
        }
        insert_custom(&conn, &tool).map_err(|e| e.to_string())?;
        write_mirror(&conn).map_err(|e| e.to_string())?;
        Ok(custom_row(&tool))
    })
    .await;
    match result {
        Ok(Ok(row)) => {
            crate::cli_tools::reload_gizzi().await;
            (StatusCode::CREATED, Json(json!({ "tool": row }))).into_response()
        }
        Ok(Err(e)) if e == "limit" => json_error(
            StatusCode::CONFLICT,
            "too_many_tools",
            "Up to 50 custom tools",
        ),
        Ok(Err(error)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", error),
        Err(error) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            error.to_string(),
        ),
    }
}

async fn delete_custom(
    State(state): State<Arc<AppState>>,
    Path(custom_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = get_user(&headers) else {
        return json_error(StatusCode::UNAUTHORIZED, "Unauthorized", "Sign in first");
    };
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = db.connect().map_err(|e| e.to_string())?;
        let removed =
            delete_custom_row(&conn, &user.user_id, &custom_id).map_err(|e| e.to_string())?;
        if removed {
            write_mirror(&conn).map_err(|e| e.to_string())?;
        }
        Ok(removed)
    })
    .await;
    match result {
        Ok(Ok(true)) => {
            crate::cli_tools::reload_gizzi().await;
            Json(json!({ "ok": true })).into_response()
        }
        Ok(Ok(false)) => json_error(StatusCode::NOT_FOUND, "not_found", "No such custom tool"),
        Ok(Err(error)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", error),
        Err(error) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            error.to_string(),
        ),
    }
}

/// A user's custom tool by id, for the install endpoint.
pub(crate) fn load_custom(state: &AppState, user_id: &str, id: &str) -> Option<CustomTool> {
    let conn = state.db.connect().ok()?;
    get_custom(&conn, user_id, id).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(command: &str, prompt_args: &[&str]) -> CustomToolBody {
        CustomToolBody {
            name: "My Tool".into(),
            command: command.into(),
            prompt_args: prompt_args.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn catalog_parses_and_ids_are_unique() {
        let tools = tools();
        assert!(tools.len() >= 30, "catalog has {} tools", tools.len());
        let mut seen = std::collections::HashSet::new();
        for tool in tools {
            assert!(seen.insert(tool.id.as_str()), "duplicate id {}", tool.id);
            for alias in &tool.aliases {
                assert!(seen.insert(alias.as_str()), "alias {alias} collides");
            }
            assert!(
                matches!(tool.adapter.as_str(), "builtin" | "text" | "unsupported"),
                "{}: adapter",
                tool.id
            );
            assert!(
                matches!(tool.sign_in.as_str(), "subscription" | "api_key" | "none"),
                "{}: signIn",
                tool.id
            );
            assert!(valid_program(&tool.bin), "{}: bin", tool.id);
            if tool.adapter == "text" {
                let h = tool
                    .headless
                    .as_ref()
                    .unwrap_or_else(|| panic!("{}: text adapter needs headless", tool.id));
                assert_eq!(
                    h.args.iter().filter(|a| a.as_str() == "{prompt}").count(),
                    1,
                    "{}: one {{prompt}}",
                    tool.id
                );
                assert!(
                    h.args
                        .iter()
                        .all(|a| a == "{prompt}" || !a.contains("{prompt}")),
                    "{}",
                    tool.id
                );
            }
            if let Some(logo) = &tool.logo {
                assert!(
                    ["/assets/runtime-logos/", "/assets/logos-apps/", "/assets/vendors/", "/app-icons/"]
                        .iter()
                        .any(|prefix| logo.starts_with(prefix)),
                    "{}",
                    tool.id
                );
            }
        }
    }

    #[test]
    fn harness_tools_resolve_to_the_install_manifest_and_others_carry_a_recipe() {
        for tool in tools() {
            let Some(recipe) = &tool.install else {
                continue;
            };
            if let Some(key) = &recipe.harness {
                assert!(
                    crate::cli_tools::tool_info(&tool.id, &tool.bin)
                        .is_some_and(|info| &info.key == key),
                    "{}: harness key {key} must install the same binary",
                    tool.id
                );
            } else {
                assert!(
                    direct_install(tool).is_some(),
                    "{}: install recipe must be complete",
                    tool.id
                );
                assert!(
                    tool.license.is_some(),
                    "{}: direct installs declare a licence",
                    tool.id
                );
            }
        }
    }

    #[test]
    fn aliases_find_catalog_tools() {
        assert_eq!(find("claude").map(|t| t.id.as_str()), Some("claude-cli"));
        assert_eq!(find("codex-cli").map(|t| t.bin.as_str()), Some("codex"));
        assert!(find("not-a-cli").is_none());
    }

    #[test]
    fn npm_recipes_install_pinned_into_the_harness_prefix() {
        let tool: CliTool = serde_json::from_value(json!({
            "id": "x", "name": "X", "vendor": "V", "bin": "x", "adapter": "text", "signIn": "none",
            "headless": {"args": ["-p", "{prompt}"]},
            "install": {"method": "npm", "package": "@v/x", "pinnedVersion": "1.2.3"}, "license": "mit"
        }))
        .unwrap();
        match direct_install(&tool) {
            Some(DirectInstall::Argv { program, args, .. }) => {
                assert_eq!(program, "npm");
                assert_eq!(args.last().map(String::as_str), Some("@v/x@1.2.3"));
                assert!(args.contains(&"--prefix".to_string()));
            }
            other => panic!("unexpected {other:?}"),
        }
        let script: CliTool = serde_json::from_value(json!({
            "id": "y", "name": "Y", "vendor": "V", "bin": "y", "adapter": "text", "signIn": "none",
            "install": {"method": "script", "url": "http://insecure.example/install.sh"}
        }))
        .unwrap();
        assert_eq!(
            direct_install(&script),
            None,
            "plain-http scripts are refused"
        );
    }

    #[test]
    fn custom_tools_validate_strictly() {
        let ok =
            validate_custom(body("mytool", &["run", "--print", "{prompt}"]), "u1").expect("valid");
        assert!(ok.id.starts_with("custom-my-tool-"));
        assert_eq!(ok.user_id, "u1");
        assert!(validate_custom(body("/opt/My Tools/bin/agent", &["{prompt}"]), "u1").is_ok());

        for bad in [
            "my tool",
            "rm;ls",
            "$(evil)",
            "a|b",
            "`x`",
            "-x",
            "../bin/x",
            "/opt/../x",
            "~/bin/x",
            "",
        ] {
            assert!(
                validate_custom(body(bad, &["{prompt}"]), "u1").is_err(),
                "command {bad:?} must be refused"
            );
        }
        assert!(
            validate_custom(body("t", &["-p"]), "u1").is_err(),
            "needs {{prompt}}"
        );
        assert!(
            validate_custom(body("t", &["{prompt}", "{prompt}"]), "u1").is_err(),
            "only one prompt"
        );
        assert!(
            validate_custom(body("t", &["--q={prompt}"]), "u1").is_err(),
            "prompt is its own argv element"
        );

        let mut with_model = body("t", &["{prompt}"]);
        with_model.model_args = Some(vec!["--model".into()]);
        assert!(
            validate_custom(with_model, "u1").is_err(),
            "modelArgs need {{model}}"
        );
        let mut with_model = body("t", &["{prompt}"]);
        with_model.model_args = Some(vec!["--model={model}".into()]);
        assert!(validate_custom(with_model, "u1").is_ok());

        let mut install = body("t", &["{prompt}"]);
        install.install_command = Some("npm install -g t@1.0.0".into());
        assert!(validate_custom(install, "u1").is_ok());
        for bad in [
            "curl https://x | sh",
            "npm i x && rm -rf /",
            "bash install.sh",
            "npm i $X",
        ] {
            let mut install = body("t", &["{prompt}"]);
            install.install_command = Some(bad.into());
            assert!(
                validate_custom(install, "u1").is_err(),
                "install {bad:?} must be refused"
            );
        }
        let mut home = body("t", &["{prompt}"]);
        home.homepage = Some("javascript:alert(1)".into());
        assert!(validate_custom(home, "u1").is_err());
    }

    #[test]
    fn custom_tools_persist_per_user_and_mirror_for_gizzi() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V269__custom_cli_tools.sql"))
            .unwrap();
        let mut a = validate_custom(body("toola", &["-p", "{prompt}"]), "alice").unwrap();
        a.model_args = vec!["-m".into(), "{model}".into()];
        let b = validate_custom(body("toolb", &["{prompt}"]), "bob").unwrap();
        insert_custom(&conn, &a).unwrap();
        insert_custom(&conn, &b).unwrap();

        assert_eq!(list_custom(&conn, Some("alice")).unwrap(), vec![a.clone()]);
        assert_eq!(list_custom(&conn, None).unwrap().len(), 2);
        assert!(
            get_custom(&conn, "bob", &a.id).unwrap().is_none(),
            "users can't see each other's tools"
        );
        assert!(
            !delete_custom_row(&conn, "bob", &a.id).unwrap(),
            "or delete them"
        );

        let mirror = mirror_json(&list_custom(&conn, None).unwrap());
        let first = &mirror["tools"][0];
        assert_eq!(first["bin"], "toola");
        assert_eq!(first["adapter"], "text");
        assert_eq!(first["headless"]["args"], json!(["-p", "{prompt}"]));
        assert_eq!(first["headless"]["modelArgs"], json!(["-m", "{model}"]));

        assert!(delete_custom_row(&conn, "alice", &a.id).unwrap());
        assert!(list_custom(&conn, Some("alice")).unwrap().is_empty());
    }

    #[test]
    fn custom_rows_report_install_state() {
        let tool = validate_custom(body("/definitely/not/here/agent", &["{prompt}"]), "u").unwrap();
        let row = custom_row(&tool);
        assert_eq!(row["installed"], false);
        assert_eq!(row["status"], "not_installed");
        assert_eq!(row["custom"], true);
        assert_eq!(row["install"], Value::Null);
    }
}
