//! `ao harness status|sync|uninstall` — Rust port of the Allternit ops harness
//! sync (`Allternit Brain/Ops/harness-sync.js` + `harness-sync/lib.js`) with
//! byte-parity across all 16 tools in `Ops/harness.json`.
//!
//! Port map: `Research/drafts/prep-p4-harness-port.md` (binding; wins over the
//! phase spec on disagreement). The manifest is the single source of truth —
//! the JS driver config bodies are dead code (gizzi drifted) and are NOT
//! ported; only each driver's `key`/`label`/`installed()` probe sequence is,
//! via the [`DRIVERS`] table (conformance-checked against the JS driver files
//! by tests/ao_harness_parity/run.sh via drivers.tsv).
//!
//! Pure filesystem CLI: no engine socket, no network. The only subprocesses
//! are the `cli`-kind MCP tool invocations (`grok`/`agy`/`opencode`/`qwen`),
//! exactly like the JS. The MCP server itself stays a node process by design.
//!
//! The manifest ships embedded below (`harness.json`, a verbatim copy of
//! `Ops/harness.json` — run.sh asserts the copy stays byte-identical).
//! `AO_HARNESS_MANIFEST` overrides the path for tests and future phases.

mod collate;
mod json_val;
mod mcp_cli;
mod mcp_json;
mod mcp_toml;
mod rules;
mod skills;

pub(crate) mod install;

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use json_val::JVal;

/// Verbatim copy of `Allternit Brain/Ops/harness.json` (checked by
/// tests/ao_harness_parity/run.sh).
const EMBEDDED_MANIFEST: &str = include_str!("harness.json");

// ---------------------------------------------------------------------------
// Manifest schema (serde: every optional defaulted, unknown keys ignored —
// `_convention`/`_notes`/`_skipped` must parse)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Manifest {
    pub(crate) source: Source,
    pub(crate) tools: BTreeMap<String, ToolCfg>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Source {
    pub(crate) skills_dir: String,
    pub(crate) rules_file: String,
    pub(crate) mcp_server: McpServer,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct McpServer {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolCfg {
    #[serde(default)]
    pub(crate) skills_dir: Option<String>,
    #[serde(default)]
    pub(crate) skills_format: Option<String>,
    #[serde(default)]
    pub(crate) rules_file: Option<String>,
    #[serde(default)]
    pub(crate) rules_frontmatter: Option<String>,
    #[serde(default)]
    pub(crate) sync_when_absent: bool,
    #[serde(default)]
    pub(crate) mcp: Option<McpCfg>,
    // P7 (ao harness install): per-tool license class + install recipe. Both
    // verified ABSENT before P7 (spec binding 2); `_licenseNote` in the
    // manifest carries the evidence URL for each class assignment.
    #[serde(default)]
    pub(crate) license: Option<LicenseClass>,
    #[serde(default)]
    pub(crate) install: Option<InstallBlock>,
}

/// License classes (plan §8 gate). `apache`/`mit`/`bsd` install without
/// ceremony; `proprietary-terms` and `undeclared` require `--accept-terms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LicenseClass {
    Apache,
    Mit,
    Bsd,
    #[serde(rename = "proprietary-terms")]
    ProprietaryTerms,
    Undeclared,
}

impl LicenseClass {
    pub(crate) fn requires_acceptance(self) -> bool {
        matches!(self, LicenseClass::ProprietaryTerms | LicenseClass::Undeclared)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LicenseClass::Apache => "apache",
            LicenseClass::Mit => "mit",
            LicenseClass::Bsd => "bsd",
            LicenseClass::ProprietaryTerms => "proprietary-terms",
            LicenseClass::Undeclared => "undeclared",
        }
    }
}

/// `{method, pinnedVersion, installArgs, verifyCmd}` — the install recipe
/// (spec binding 1). serde permissive like the rest of the manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstallBlock {
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) pinned_version: Option<String>,
    #[serde(default)]
    pub(crate) install_args: Vec<String>,
    #[serde(default)]
    pub(crate) verify_cmd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpCfg {
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) servers_key: Option<String>,
    #[serde(default)]
    pub(crate) section: Option<String>,
    #[serde(default)]
    pub(crate) bin: Option<String>,
    #[serde(default)]
    pub(crate) add_args: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) remove_args: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) config_path: Option<String>,
    #[serde(default)]
    pub(crate) config_format: Option<String>,
    #[serde(default)]
    pub(crate) command_array: Option<bool>,
    #[serde(default)]
    pub(crate) entry_type: Option<String>,
}

fn load_manifest() -> std::io::Result<Manifest> {
    let text = if let Ok(override_path) = std::env::var("AO_HARNESS_MANIFEST") {
        std::fs::read_to_string(&override_path).map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!("cannot read harness manifest {override_path}: {err}"),
            )
        })?
    } else {
        EMBEDDED_MANIFEST.to_string()
    };
    serde_json::from_str(&text)
        .map_err(|err| std::io::Error::new(ErrorKind::InvalidData, format!("invalid harness manifest: {err}")))
}

/// Doctor entry point (cli/ao.rs): load the manifest for the harness section.
pub(crate) fn load_manifest_for_doctor() -> std::io::Result<Manifest> {
    load_manifest()
}

/// Doctor entry point (cli/ao.rs): run the install doctor against the
/// ambient probe context (managed bin already joined — see FsCtx::from_env).
pub(crate) fn doctor_for_cli(manifest: &Manifest) -> install::HarnessDoctor {
    let root = install::managed_root();
    let ctx = FsCtx::from_env();
    install::doctor(manifest, &root, &ctx)
}

// ---------------------------------------------------------------------------
// Driver table — key/label/installed() only, verbatim from drivers/*.js
// (conformance-checked against the JS files by the parity harness).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub(crate) enum Probe {
    Exists(&'static str),
    Which(&'static str),
}

pub(crate) struct Driver {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    probes: &'static [Probe],
}

pub(crate) const DRIVERS: &[Driver] = &[
    Driver { key: "claude", label: "Claude Code", probes: &[Probe::Exists("~/.claude"), Probe::Which("claude")] },
    Driver { key: "codex", label: "Codex CLI", probes: &[Probe::Exists("~/.codex"), Probe::Which("codex")] },
    Driver { key: "kimi", label: "Kimi Code CLI", probes: &[Probe::Exists("~/.kimi-code"), Probe::Which("kimi")] },
    Driver { key: "grok", label: "Grok CLI", probes: &[Probe::Exists("~/.grok"), Probe::Which("grok")] },
    Driver {
        key: "cursor",
        label: "Cursor",
        probes: &[Probe::Exists("~/.cursor"), Probe::Which("cursor"), Probe::Exists("/Applications/Cursor.app")],
    },
    Driver { key: "gizzi", label: "Gizzi Code", probes: &[Probe::Exists("~/.gizzi"), Probe::Which("gizzi")] },
    Driver {
        key: "agy",
        label: "Antigravity CLI",
        probes: &[
            Probe::Which("agy"),
            Probe::Exists("~/.local/bin/agy"),
            Probe::Exists("~/.gemini/antigravity-cli"),
        ],
    },
    Driver { key: "opencode", label: "OpenCode", probes: &[Probe::Which("opencode"), Probe::Exists("~/.config/opencode")] },
    Driver { key: "qwen", label: "Qwen Code", probes: &[Probe::Which("qwen"), Probe::Exists("~/.qwen")] },
    Driver { key: "codebuddy", label: "CodeBuddy", probes: &[Probe::Which("codebuddy"), Probe::Exists("~/.codebuddy")] },
    Driver { key: "workbuddy", label: "WorkBuddy", probes: &[Probe::Which("workbuddy"), Probe::Exists("~/.workbuddy")] },
    Driver { key: "openclaw", label: "OpenClaw", probes: &[Probe::Which("openclaw"), Probe::Exists("~/.openclaw")] },
    Driver { key: "hermes", label: "Hermes", probes: &[Probe::Which("hermes"), Probe::Exists("~/.hermes")] },
    Driver { key: "dsh", label: "DeepSeek Harness", probes: &[Probe::Which("dsh"), Probe::Exists("~/.dsh")] },
    Driver { key: "pi", label: "Pi", probes: &[Probe::Which("pi"), Probe::Exists("~/.pi")] },
    Driver { key: "qoder", label: "Qoder", probes: &[Probe::Which("qoder"), Probe::Exists("~/.qoder")] },
];

/// Detection context: home dir, PATH entries, and the root absolute probe
/// paths resolve against (`/` in production; a scratch root in tests so the
/// `/Applications/Cursor.app` probe is testable without touching /Applications).
pub(crate) struct FsCtx {
    home: PathBuf,
    path_dirs: Vec<PathBuf>,
    app_root: PathBuf,
}

impl FsCtx {
    fn from_env() -> Self {
        // P7: a managed-dir install (AO_HARNESS_HOME or ~/.ao/harness) must be
        // visible to the installed() probes so `ao harness sync` reaches tools
        // ao itself installed — the managed bin dir joins PATH exactly the way
        // HR CE puts $TOOLS/bin on PATH for its entrypoint (spec binding 5:
        // executor registration is P4 machinery; P7 just feeds it a reachable
        // binary). The user's own shell rc is never touched.
        let mut path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).collect())
            .unwrap_or_default();
        let managed_bin = install::managed_bin_dir();
        if managed_bin.is_dir() && !path_dirs.iter().any(|d| d == &managed_bin) {
            path_dirs.insert(0, managed_bin);
        }
        FsCtx {
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")),
            path_dirs,
            app_root: PathBuf::from("/"),
        }
    }

    fn expand_probe(&self, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix('~') {
            // Naive leading-~ expansion, matching lib.js expand() +
            // path.join(): "~user" collapses to $HOME/user (reproduced).
            PathBuf::from(format!("{}{rest}", self.home.display()))
        } else {
            self.app_root.join(path.trim_start_matches('/'))
        }
    }

    fn which(&self, bin: &str) -> bool {
        self.path_dirs.iter().any(|dir| is_executable(&dir.join(bin)))
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).map(|meta| meta.is_file()).unwrap_or(false)
}

impl Driver {
    fn installed(&self, ctx: &FsCtx) -> bool {
        self.probes.iter().any(|probe| match probe {
            Probe::Exists(path) => ctx.expand_probe(path).exists(),
            Probe::Which(bin) => ctx.which(bin),
        })
    }
}

fn absent_skips_sync(driver: &Driver, cfg: &ToolCfg, ctx: &FsCtx) -> bool {
    !driver.installed(ctx) && !cfg.sync_when_absent
}

// ---------------------------------------------------------------------------
// Shared filesystem helpers (lib.js ports)
// ---------------------------------------------------------------------------

/// `expand()`: naive leading-`~` → $HOME. Node's path.join would also
/// normalize (collapse `//`, resolve `..`); manifest/fixture paths are clean,
/// so plain concatenation is used (documented in the P4 notes).
pub(crate) fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        PathBuf::from(format!("{home}{rest}"))
    } else {
        PathBuf::from(p)
    }
}

pub(crate) fn exists(p: &str) -> bool {
    expand(p).exists()
}

/// `readJson()`: strict parse; ANY error → None (the JS fragility around
/// `.jsonc` files containing comments is reproduced deliberately).
pub(crate) fn read_json(file: &str) -> Option<JVal> {
    let text = std::fs::read_to_string(expand(file)).ok()?;
    json_val::parse(&text).ok()
}

/// `writeText()`: mkdir parent, back up an existing file to
/// `<file>.bak-harness` (parity side effect), then write.
pub(crate) fn write_text(file: &str, content: &str) -> std::io::Result<()> {
    let f = expand(file);
    if let Some(parent) = f.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if f.exists() {
        std::fs::copy(&f, f.with_file_name(format!(
            "{}.bak-harness",
            f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        )))?;
    }
    std::fs::write(&f, content)
}

/// `writeJson()`: full-file pretty rewrite, `JSON.stringify(obj, null, 2) + "\n"`.
pub(crate) fn write_json(file: &str, value: &JVal) -> std::io::Result<()> {
    write_text(file, &format!("{}\n", json_val::print_pretty(value)))
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

/// `new Date().toISOString()` — wall-clock UTC, millisecond precision.
/// Excluded from the byte-parity diff (see the parity harness).
pub(crate) fn iso8601_now() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = millis / 1000;
    let ms = millis % 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil_from_days (proleptic Gregorian calendar).
fn civil_from_days(z: i64) -> (u32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as u32, m, d)
}

// ---------------------------------------------------------------------------
// Action (mirrors the JS action objects; kinds are strings because dry-run
// renames them: install → would-install, ...)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct Action {
    pub(crate) kind: String,
    pub(crate) name: Option<String>,
    pub(crate) detail: Option<String>,
    pub(crate) reason: Option<String>,
}

impl Action {
    pub(crate) fn new(kind: impl Into<String>) -> Action {
        Action { kind: kind.into(), name: None, detail: None, reason: None }
    }

    pub(crate) fn named(kind: impl Into<String>, name: impl Into<String>) -> Action {
        Action { name: Some(name.into()), ..Action::new(kind) }
    }

    pub(crate) fn detailed(kind: impl Into<String>, detail: impl Into<String>) -> Action {
        Action { detail: Some(detail.into()), ..Action::new(kind) }
    }

    pub(crate) fn unchanged() -> Action {
        Action::new("unchanged")
    }

    pub(crate) fn nothing(detail: impl Into<String>) -> Action {
        Action::detailed("nothing", detail)
    }
}

fn describe(a: &Action) -> String {
    if a.kind == "unchanged" {
        return "unchanged".to_string();
    }
    if a.kind == "nothing" {
        return format!("nothing ({})", a.detail.as_deref().unwrap_or("n/a"));
    }
    let mut bits = vec![a.kind.clone()];
    if let Some(name) = &a.name {
        bits.push(name.clone());
    }
    if let Some(detail) = &a.detail {
        bits.push(format!("— {detail}"));
    }
    if let Some(reason) = &a.reason {
        bits.push(format!("({reason})"));
    }
    bits.join(" ")
}

// ---------------------------------------------------------------------------
// MCP dispatch (harness-sync.js mcpStatus/mcpSync/mcpRemove ports)
// ---------------------------------------------------------------------------

fn mcp_status(m: &McpCfg, server: &McpServer) -> std::io::Result<&'static str> {
    match m.kind.as_str() {
        "json" => mcp_json::status(m.path.as_deref().unwrap_or_default(), m, &server.name, server),
        "toml-block" => mcp_toml::status(
            m.path.as_deref().unwrap_or_default(),
            m.section.as_deref().unwrap_or_default(),
            server,
        ),
        "cli" => {
            if m.config_format.as_deref() == Some("json") {
                mcp_json::status(
                    m.config_path.as_deref().unwrap_or_default(),
                    m,
                    &server.name,
                    server,
                )
            } else {
                mcp_toml::status(
                    m.config_path.as_deref().unwrap_or_default(),
                    m.section.as_deref().unwrap_or_default(),
                    server,
                )
            }
        }
        _ => Ok("unknown"),
    }
}

fn mcp_sync(m: &McpCfg, server: &McpServer, dry_run: bool) -> std::io::Result<Vec<Action>> {
    match m.kind.as_str() {
        "json" => mcp_json::sync(m.path.as_deref().unwrap_or_default(), m, &server.name, server, dry_run),
        "toml-block" => mcp_toml::sync(
            m.path.as_deref().unwrap_or_default(),
            m.section.as_deref().unwrap_or_default(),
            server,
            dry_run,
        ),
        "cli" => {
            // Sync quirk (harness-sync.js:87-90): status is checked against
            // configPath FIRST — `ok` → unchanged, never shells out.
            if mcp_status(m, server)? == "ok" {
                return Ok(vec![Action::unchanged()]);
            }
            mcp_cli::cli_sync(m.bin.as_deref().unwrap_or_default(), m.add_args.as_deref().unwrap_or_default(), dry_run)
        }
        _ => Ok(vec![Action::new("unknown")]),
    }
}

fn mcp_remove(m: &McpCfg, server: &McpServer, dry_run: bool) -> std::io::Result<Vec<Action>> {
    match m.kind.as_str() {
        "json" => mcp_json::remove(m.path.as_deref().unwrap_or_default(), m, &server.name, dry_run),
        "toml-block" => mcp_toml::remove(
            m.path.as_deref().unwrap_or_default(),
            m.section.as_deref().unwrap_or_default(),
            dry_run,
        ),
        "cli" => {
            if m.config_format.as_deref() == Some("json") {
                // Direct file edit regardless of removeArgs (opencode has
                // removeArgs: null and edits opencode.jsonc directly).
                mcp_json::remove(m.config_path.as_deref().unwrap_or_default(), m, &server.name, dry_run)
            } else {
                Ok(mcp_cli::cli_remove(
                    m.bin.as_deref().unwrap_or_default(),
                    m.remove_args.as_deref().unwrap_or_default(),
                    dry_run,
                ))
            }
        }
        _ => Ok(vec![Action::new("unknown")]),
    }
}

// ---------------------------------------------------------------------------
// CLI surface (`ao harness [status|sync|uninstall] [--dry-run] [--tools=a,b]`)
// ---------------------------------------------------------------------------

pub(crate) fn run(args: &[String]) -> std::io::Result<i32> {
    // harness-sync.js argument model: first non--- token is the command
    // (default status); flags may appear in any order. P7 adds `install`,
    // whose tool names are positional tokens after the command, plus
    // `--accept-terms <tool>` (value option — its value must not be mistaken
    // for a positional).
    let mut accepts_terms: Vec<String> = Vec::new();
    let mut positionals: Vec<String> = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if let Some(tool) = arg.strip_prefix("--accept-terms=") {
            accepts_terms.push(tool.to_string());
        } else if arg == "--accept-terms" {
            match iter.next() {
                Some(tool) => accepts_terms.push(tool.clone()),
                None => {
                    eprintln!("--accept-terms requires a tool name");
                    return Ok(2);
                }
            }
        } else if !arg.starts_with("--") {
            positionals.push(arg.clone());
        }
    }
    let command = positionals.first().map(String::as_str).unwrap_or("status");
    // `positionals.get(1..)` — a bare `ao harness` (no positional at all)
    // must not slice-panic on an empty vec.
    let install_tools: Vec<&str> = positionals
        .get(1..)
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .collect();
    let dry_run = args.iter().any(|arg| arg == "--dry-run");
    let tools_filter: Vec<&str> = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--tools="))
        .map(|list| list.split(',').filter(|key| !key.is_empty()).collect())
        .unwrap_or_default();

    let manifest = load_manifest()?;
    if !exists(&manifest.source.skills_dir) {
        eprintln!("Source skills dir not found: {}", manifest.source.skills_dir);
        return Ok(1);
    }

    let ctx = FsCtx::from_env();
    match command {
        "status" => cmd_status(&manifest, &tools_filter, &ctx),
        "sync" => cmd_sync(&manifest, &tools_filter, dry_run, &ctx),
        "uninstall" => cmd_uninstall(&manifest, &tools_filter, dry_run),
        "install" => install::cmd_install(&manifest, &install_tools, &accepts_terms, dry_run),
        other => {
            // Byte-parity: the JS harness-sync prints exactly this list; the
            // parity harness diffs the error line (ao_harness_parity/run.sh).
            eprintln!("Unknown command: {other} (expected status | sync | uninstall)");
            Ok(1)
        }
    }
}

fn selected_tools<'m>(
    manifest: &'m Manifest,
    tools_filter: &[&str],
) -> Vec<(&'static Driver, &'m ToolCfg)> {
    DRIVERS
        .iter()
        .filter_map(|driver| manifest.tools.get(driver.key).map(|cfg| (driver, cfg)))
        .filter(|(driver, _)| tools_filter.is_empty() || tools_filter.contains(&driver.key))
        .collect()
}

fn skills_format(cfg: &ToolCfg) -> skills::SkillsFormat {
    if cfg.skills_format.as_deref() == Some("flat") {
        skills::SkillsFormat::Flat
    } else {
        skills::SkillsFormat::Dir
    }
}

fn cmd_status(manifest: &Manifest, tools_filter: &[&str], ctx: &FsCtx) -> std::io::Result<i32> {
    let skills_count = skills::list_skills(&manifest.source.skills_dir)?.len();
    println!(
        "Harness source: {} ({} skills), rules: {}",
        manifest.source.skills_dir, skills_count, manifest.source.rules_file
    );
    let server = &manifest.source.mcp_server;
    println!(
        "MCP server: {} → {} {}",
        server.name,
        server.command,
        server.args.first().map(String::as_str).unwrap_or_default()
    );
    println!();

    struct Row {
        tool: String,
        installed: &'static str,
        skills: String,
        mcp: &'static str,
        rules: &'static str,
    }

    let mut rows = Vec::new();
    for (driver, cfg) in selected_tools(manifest, tools_filter) {
        if absent_skips_sync(driver, cfg, ctx) {
            rows.push(Row {
                tool: driver.label.to_string(),
                installed: "no",
                skills: "skipped".to_string(),
                mcp: "skipped",
                rules: "skipped",
            });
            continue;
        }
        rows.push(Row {
            tool: driver.label.to_string(),
            installed: "yes",
            skills: match &cfg.skills_dir {
                Some(dir) => skills::skills_status(
                    &manifest.source.skills_dir,
                    dir,
                    skills_format(cfg),
                )?,
                None => "n/a".to_string(),
            },
            mcp: match &cfg.mcp {
                Some(m) => mcp_status(m, &manifest.source.mcp_server)?,
                None => "n/a",
            },
            rules: match &cfg.rules_file {
                Some(file) => rules::rules_block_status(file)?,
                None => "n/a",
            },
        });
    }

    let w_tool = rows.iter().map(|r| r.tool.chars().count()).max().unwrap_or(0).max(4);
    let w_installed = 9;
    let w_skills = rows.iter().map(|r| r.skills.chars().count()).max().unwrap_or(0).max(6);
    let w_mcp = rows.iter().map(|r| r.mcp.chars().count()).max().unwrap_or(0).max(3);
    let w_rules = rows.iter().map(|r| r.rules.chars().count()).max().unwrap_or(0).max(5);

    println!(
        "{:<w_tool$}  {:<w_installed$}  {:<w_skills$}  {:<w_mcp$}  rules",
        "tool", "installed", "skills", "mcp"
    );
    println!(
        "{}  {}  {}  {}  {}",
        "-".repeat(w_tool),
        "-".repeat(w_installed),
        "-".repeat(w_skills),
        "-".repeat(w_mcp),
        "-".repeat(w_rules)
    );
    for row in &rows {
        println!(
            "{:<w_tool$}  {:<w_installed$}  {:<w_skills$}  {:<w_mcp$}  {}",
            row.tool, row.installed, row.skills, row.mcp, row.rules
        );
    }
    println!();
    println!("mcp: ok = registered with correct command/args · broken = registered but stale · missing = not registered");
    Ok(0)
}

fn cmd_sync(manifest: &Manifest, tools_filter: &[&str], dry_run: bool, ctx: &FsCtx) -> std::io::Result<i32> {
    let rules_content = std::fs::read_to_string(expand(&manifest.source.rules_file))?;
    let server = &manifest.source.mcp_server;
    let mut total = 0usize;
    for (driver, cfg) in selected_tools(manifest, tools_filter) {
        println!("\n== {} ==", driver.label);
        if absent_skips_sync(driver, cfg, ctx) {
            println!("   not installed (skipped) — install the tool and re-run sync to pick it up");
            continue;
        }
        if !driver.installed(ctx) {
            println!("   (tool not detected on this machine — writing config anyway: syncWhenAbsent)");
        }
        if let Some(dir) = &cfg.skills_dir {
            for action in skills::sync_skills(&manifest.source.skills_dir, dir, dry_run, skills_format(cfg))? {
                println!("   skills: {}", describe(&action));
                total += 1;
            }
        }
        if let Some(m) = &cfg.mcp {
            for action in mcp_sync(m, server, dry_run)? {
                println!("   mcp:    {}", describe(&action));
                total += 1;
            }
        }
        if let Some(file) = &cfg.rules_file {
            let block = rules::build_rules_block(&rules_content);
            for action in rules::upsert_rules_block(file, &block, dry_run, cfg.rules_frontmatter.as_deref())? {
                println!("   rules:  {}", describe(&action));
                total += 1;
            }
        }
    }
    println!(
        "\n{} {} action(s) reported.",
        if dry_run { "Dry run — no changes written." } else { "Done." },
        total
    );
    Ok(0)
}

fn cmd_uninstall(manifest: &Manifest, tools_filter: &[&str], dry_run: bool) -> std::io::Result<i32> {
    let server = &manifest.source.mcp_server;
    for (driver, cfg) in selected_tools(manifest, tools_filter) {
        println!("\n== {} ==", driver.label);
        // Uninstall deliberately has no absent-tool skip (harness-sync.js:184).
        if let Some(dir) = &cfg.skills_dir {
            for action in skills::uninstall_skills(dir, dry_run, skills_format(cfg))? {
                println!("   skills: {}", describe(&action));
            }
        }
        if let Some(m) = &cfg.mcp {
            for action in mcp_remove(m, server, dry_run)? {
                println!("   mcp:    {}", describe(&action));
            }
        }
        if let Some(file) = &cfg.rules_file {
            for action in rules::remove_rules_block(file, dry_run, cfg.rules_frontmatter.as_deref())? {
                println!("   rules:  {}", describe(&action));
            }
        }
    }
    println!(
        "\n{}",
        if dry_run {
            "Dry run — nothing removed."
        } else {
            "Uninstall complete. Only harness-managed resources were touched."
        }
    );
    Ok(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_parses_with_all_16_tools() {
        let manifest: Manifest = serde_json::from_str(EMBEDDED_MANIFEST).expect("embedded manifest parses");
        assert_eq!(manifest.tools.len(), 16, "manifest must cover exactly 16 tools");
        for driver in DRIVERS {
            assert!(manifest.tools.contains_key(driver.key), "manifest missing tool {}", driver.key);
        }
        assert_eq!(DRIVERS.len(), 16);
    }

    #[test]
    fn every_tool_carries_license_and_install_blocks() {
        // P7 conformance gate (spec verify plan #2): all 16 tools carry a
        // license class + install recipe; the parity harness keeps the
        // embedded copy byte-identical to Ops/harness.json.
        let manifest: Manifest = serde_json::from_str(EMBEDDED_MANIFEST).expect("embedded manifest parses");
        for (key, cfg) in &manifest.tools {
            let class = cfg
                .license
                .unwrap_or_else(|| panic!("{key} missing license tag"));
            let block = cfg.install.as_ref().unwrap_or_else(|| panic!("{key} missing install block"));
            match block.method.as_str() {
                "npm" | "venv-pip" | "script" => {
                    assert!(
                        block.pinned_version.as_deref().is_some_and(|p| !p.is_empty()),
                        "{key}: {method} method must carry a pinnedVersion",
                        method = block.method
                    );
                    assert!(!block.install_args.is_empty(), "{key}: installArgs empty");
                    assert!(
                        block.verify_cmd.as_deref().is_some_and(|c| !c.is_empty()),
                        "{key}: verifyCmd required"
                    );
                }
                "unsupported" => {}
                other => panic!("{key}: unknown install method {other}"),
            }
            if class.requires_acceptance() {
                assert!(
                    matches!(block.method.as_str(), "npm" | "venv-pip" | "script" | "unsupported"),
                    "{key}: gated class with unexpected method"
                );
            }
        }
        // Spot-check the spec-named gate assignments.
        let class_of = |key: &str| manifest.tools[key].license.unwrap();
        assert_eq!(class_of("claude"), LicenseClass::ProprietaryTerms);
        assert_eq!(class_of("hermes"), LicenseClass::Undeclared);
        assert_eq!(class_of("dsh"), LicenseClass::Undeclared);
        assert_eq!(class_of("kimi"), LicenseClass::Mit);
        assert_eq!(class_of("codex"), LicenseClass::Apache);
    }

    #[test]
    fn driver_table_matches_checked_in_tsv() {
        // Conformance gate for the gizzi-drift class of bug: the Rust
        // key/label/probe table must match tests/ao_harness_parity/drivers.tsv,
        // which run.sh diffs against the live JS driver files.
        let tsv = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/ao_harness_parity/drivers.tsv"
        ))
        .expect("drivers.tsv exists");
        let expected: Vec<Vec<String>> = tsv
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.split('\t').map(str::to_string).collect())
            .collect();
        assert_eq!(expected.len(), DRIVERS.len(), "driver count mismatch");
        for (row, driver) in expected.iter().zip(DRIVERS.iter()) {
            assert_eq!(row[0], driver.key, "key mismatch");
            assert_eq!(row[1], driver.label, "label mismatch for {}", driver.key);
            let probes: Vec<String> = driver
                .probes
                .iter()
                .map(|probe| match probe {
                    Probe::Exists(path) => format!("exists:{path}"),
                    Probe::Which(bin) => format!("which:{bin}"),
                })
                .collect();
            let tsv_probes: Vec<&str> = row[2..].iter().map(String::as_str).collect();
            assert_eq!(probes, tsv_probes, "probe mismatch for {}", driver.key);
        }
    }

    #[test]
    fn detection_probes_follow_driver_order() {
        let tmp = std::env::temp_dir().join(format!("ao-harness-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("home");
        let bin_dir = tmp.join("bin");
        let app_root = tmp.join("approot");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(&home.join(".claude")).unwrap();
        std::fs::create_dir_all(app_root.join("Applications/Cursor.app")).unwrap();
        let agy = bin_dir.join("agy");
        std::fs::write(&agy, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let ctx = FsCtx { home, path_dirs: vec![bin_dir], app_root };

        let by_key = |key: &str| DRIVERS.iter().find(|d| d.key == key).unwrap();
        assert!(by_key("claude").installed(&ctx));
        assert!(!by_key("codex").installed(&ctx));
        assert!(by_key("cursor").installed(&ctx), "/Applications probe via app_root");
        assert!(by_key("agy").installed(&ctx), "which probe finds fake agy");
        assert!(!by_key("kimi").installed(&ctx));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn iso8601_matches_ecmascript_shape() {
        let iso = iso8601_now();
        assert!(
            regex_free_iso_check(&iso),
            "unexpected shape: {iso}"
        );
    }

    fn regex_free_iso_check(iso: &str) -> bool {
        let bytes = iso.as_bytes();
        let digits = |i: usize| bytes[i].is_ascii_digit();
        let fixed = |i: usize, c: char| bytes[i] == c as u8;
        iso.len() == 24
            && (0..4).all(digits)
            && fixed(4, '-')
            && (5..7).all(digits)
            && fixed(7, '-')
            && (8..10).all(digits)
            && fixed(10, 'T')
            && (11..13).all(digits)
            && fixed(13, ':')
            && (14..16).all(digits)
            && fixed(16, ':')
            && (17..19).all(digits)
            && fixed(19, '.')
            && (20..23).all(digits)
            && fixed(23, 'Z')
    }
}
