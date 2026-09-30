//! Spawn gate: Gate 2 (`Gate::pre_tool`) plus a hard floor, injected into the
//! third-party harnesses Allternit spawns (audit S1 parts 2–3).
//!
//! Before this module, every spawned harness ran in full-bypass mode and Gate 2
//! was a CLI call the agent had to choose to make. Now:
//!
//! * Harnesses with a PreToolUse hook mechanism (Claude Code) get a
//!   session-scoped settings file whose hook runs
//!   `allternit-commrails hook claude-pretool`, which evaluates every tool
//!   call against the hard floor and, when a WIH is bound, against Gate 2 and
//!   the WIH's own lease. Denials are written to the ledger.
//! * Harnesses without one are classified here ([`HarnessGate`]); spawn paths
//!   call [`admit`] and refuse to run them on a WIH whose policy requires
//!   lease coverage for writes, because nothing could enforce it.

pub mod floor;
pub mod shell;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};

use crate::core::ids::create_event_id;
use crate::core::types::{Actor, ActorType, AllternitEvent, EventScope, LedgerQuery};
use crate::gate::Gate;
use crate::leases::Leases;
use crate::ledger::Ledger;

use shell::Target;

/// Ledger event type for every hook decision that is recorded.
pub const HOOK_EVENT: &str = "HarnessToolGated";
/// Ledger event type for a refused spawn.
pub const SPAWN_REFUSED_EVENT: &str = "HarnessSpawnRefused";

/// How a harness is held to Allternit policy once spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessGate {
    /// Every tool call passes through the Allternit PreToolUse hook
    /// (hard floor + Gate 2 when a WIH is bound).
    Hook,
    /// No hook, but an ACP server: when driven over ACP the client answers
    /// every `session/request_permission` with the gate's verdict
    /// (gizzi-code `acp-gate.ts`), so nothing waits on a person.
    Acp,
    /// No hook; runs auto-approve inside Allternit's execution environment
    /// (worktree, env allowlist, egress guard). Not lease-precise.
    Sandbox,
    /// Runs with the vendor's auto-approve flag; only Allternit's execution
    /// environment (worktree, env allowlist, egress guard) sits in front of it.
    Ungated,
}

impl HarnessGate {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Acp => "acp",
            Self::Sandbox => "sandbox",
            Self::Ungated => "ungated",
        }
    }
}

/// Classify a harness by its base / vendor / binary name.
pub fn harness_gate(harness: &str) -> HarnessGate {
    let name = shell::basename(harness.trim()).to_ascii_lowercase();
    match name.as_str() {
        "claude" | "claude-code" => HarnessGate::Hook,
        "codex" | "qwen" | "qwen-code" => HarnessGate::Hook,
        "kimi" | "kimi-code" | "gemini" => HarnessGate::Acp,
        _ => HarnessGate::Ungated,
    }
}

/// The parts of a WIH's policy the spawn gate needs.
#[derive(Debug, Clone, PartialEq)]
pub struct WihPolicy {
    pub wih_id: String,
    /// `policy.requires_lease_for_write`. `None` when the WIH or its policy
    /// could not be found — treated as `true` (fail closed).
    pub requires_lease_for_write: Option<bool>,
}

impl WihPolicy {
    pub fn writes_need_lease(&self) -> bool {
        self.requires_lease_for_write.unwrap_or(true)
    }
}

/// Read a WIH's policy from its `WIHCreated` ledger event.
pub async fn load_wih_policy(ledger: &Ledger, wih_id: &str) -> Result<WihPolicy> {
    let events = ledger
        .query(LedgerQuery {
            r#type: Some("WIHCreated".to_string()),
            ..Default::default()
        })
        .await?;
    let requires = events
        .iter()
        .find(|e| e.payload.get("wih_id").and_then(Value::as_str) == Some(wih_id))
        .and_then(|e| e.payload.get("policy"))
        .and_then(|p| p.get("requires_lease_for_write"))
        .and_then(Value::as_bool);
    Ok(WihPolicy {
        wih_id: wih_id.to_string(),
        requires_lease_for_write: requires,
    })
}

/// Spawn admission. Every harness is admitted and launched in its own
/// auto-approve mode (Eoj, 2026-09-30): Allternit's gate is the gate. A CLI
/// held out of auto-approve asks for permission on its own side, and a
/// headless turn then hangs waiting for an answer nobody streams back.
/// The returned [`HarnessGate`] records what enforcement sits in front of it:
/// the PreToolUse hook (S0 floor + Gate 2) for hooked harnesses, and the
/// execution environment (worktree, env allowlist, egress guard) for the rest.
pub fn admit(harness: &str, _wih: Option<&WihPolicy>) -> std::result::Result<HarnessGate, String> {
    Ok(harness_gate(harness))
}

/// A PreToolUse request, normalized across harnesses (Claude Code and codex
/// both send `{tool_name, tool_input, cwd, session_id}`).
#[derive(Debug, Clone)]
pub struct HookRequest {
    pub tool_name: String,
    pub tool_input: Value,
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,
}

impl HookRequest {
    pub fn from_json(value: &Value) -> Result<Self> {
        let tool_name = value
            .get("tool_name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("hook input has no tool_name"))?
            .to_string();
        Ok(Self {
            tool_name,
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
            cwd: value.get("cwd").and_then(Value::as_str).map(PathBuf::from),
            session_id: value.get("session_id").and_then(Value::as_str).map(String::from),
        })
    }

    fn is_shell(&self) -> bool {
        matches!(
            self.tool_name.as_str(),
            "Bash" | "run_shell_command" | "shell" | "local_shell" | "exec_command" | "container.exec" | "unified_exec"
        )
    }

    /// The shell command of a shell tool call (string, or codex argv array).
    pub fn command(&self) -> Option<String> {
        if !self.is_shell() {
            return None;
        }
        match self.tool_input.get("command").or_else(|| self.tool_input.get("cmd")) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(parts)) => {
                let words: Vec<String> = parts.iter().filter_map(|p| p.as_str().map(String::from)).collect();
                // `["bash", "-lc", "<script>"]` — the script is the command.
                if words.len() == 3 && shell::inner_script(&words).is_some() {
                    Some(words[2].clone())
                } else {
                    Some(words.join(" "))
                }
            }
            _ => None,
        }
    }

    /// Paths this tool call writes, resolved against its cwd.
    pub fn write_targets(&self, home: Option<&Path>) -> Vec<Target> {
        let cwd = self.cwd.clone().unwrap_or_else(|| PathBuf::from("/"));
        if let Some(cmd) = self.command() {
            return shell::write_targets(&cmd, &cwd, home);
        }
        let mut out = Vec::new();
        for key in ["file_path", "notebook_path", "path"] {
            if matches!(self.tool_name.as_str(), "Read" | "Glob" | "Grep" | "LS" | "WebFetch" | "WebSearch") {
                break;
            }
            if let Some(p) = self.tool_input.get(key).and_then(Value::as_str) {
                out.push(shell::resolve(p, &cwd, home));
            }
        }
        if self.tool_name == "apply_patch" {
            let body = self
                .tool_input
                .get("input")
                .or_else(|| self.tool_input.get("patch"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            for line in body.lines() {
                for prefix in ["*** Add File: ", "*** Update File: ", "*** Delete File: ", "*** Move to: "] {
                    if let Some(p) = line.strip_prefix(prefix) {
                        out.push(shell::resolve(p.trim(), &cwd, home));
                    }
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow(String),
    Deny(String),
}

impl Verdict {
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny(_))
    }
    pub fn reason(&self) -> &str {
        match self {
            Self::Allow(r) | Self::Deny(r) => r,
        }
    }
}

/// A WIH bound to the spawned session: Gate 2 plus the WIH's own leases.
pub struct WihBinding<'a> {
    pub wih_id: &'a str,
    pub gate: &'a Gate,
    pub leases: &'a Leases,
}

/// Outcome of one hook evaluation.
#[derive(Debug, Clone)]
pub struct Decision {
    pub verdict: Verdict,
    /// Root-relative (or absolute, when outside root) write paths evaluated.
    pub paths: Vec<String>,
}

/// Evaluate one PreToolUse request. Order: hard floor (always) → write-target
/// resolution → Gate 2 `pre_tool` → WIH-own lease coverage. Every error path
/// is a deny.
pub async fn decide(req: &HookRequest, root: &Path, home: Option<&Path>, wih: Option<WihBinding<'_>>) -> Decision {
    if let Some(cmd) = req.command() {
        if let Some(reason) = floor::check(&cmd, home) {
            return Decision {
                verdict: Verdict::Deny(reason),
                paths: Vec::new(),
            };
        }
    }
    let Some(wih) = wih else {
        return Decision {
            verdict: Verdict::Allow("hard floor passed (no WIH bound)".to_string()),
            paths: Vec::new(),
        };
    };

    let root_forms = path_forms(root);
    let mut rel_paths = Vec::new();
    for target in req.write_targets(home) {
        match target {
            Target::Unresolved(raw) => {
                return Decision {
                    verdict: Verdict::Deny(format!(
                        "cannot resolve write target `{raw}`, so WIH {} lease coverage cannot be checked",
                        wih.wih_id
                    )),
                    paths: rel_paths,
                };
            }
            Target::Path(p) => match relative_to_root(&p, &root_forms) {
                Some(rel) => rel_paths.push(rel),
                None => {
                    rel_paths.push(p.to_string_lossy().to_string());
                    return Decision {
                        verdict: Verdict::Deny(format!(
                            "write outside WIH {} lease: {} is outside {}",
                            wih.wih_id,
                            p.display(),
                            root.display()
                        )),
                        paths: rel_paths,
                    };
                }
            },
        }
    }

    match wih.gate.pre_tool(wih.wih_id, &req.tool_name, &rel_paths).await {
        Ok(res) if !res.allowed => {
            return Decision {
                verdict: Verdict::Deny(format!(
                    "Gate 2 denied {} for WIH {}: {}",
                    req.tool_name,
                    wih.wih_id,
                    res.reason.unwrap_or_else(|| "no reason".to_string())
                )),
                paths: rel_paths,
            }
        }
        Ok(_) => {}
        Err(err) => {
            return Decision {
                verdict: Verdict::Deny(format!("Gate 2 error for WIH {} (fail closed): {err}", wih.wih_id)),
                paths: rel_paths,
            }
        }
    }

    // Gate 2's coverage check accepts any granted lease; the spawned session
    // may only write inside leases held by its own WIH.
    if !rel_paths.is_empty() {
        let own = match wih.leases.active_paths_for_wih(wih.wih_id).await {
            Ok(own) => own,
            Err(err) => {
                return Decision {
                    verdict: Verdict::Deny(format!("lease lookup failed for WIH {} (fail closed): {err}", wih.wih_id)),
                    paths: rel_paths,
                }
            }
        };
        if let Some(uncovered) = rel_paths.iter().find(|p| !own.iter().any(|l| lease_matches(l, p))) {
            return Decision {
                verdict: Verdict::Deny(format!(
                    "write outside WIH {} lease: {uncovered} is not covered by a lease this WIH holds",
                    wih.wih_id
                )),
                paths: rel_paths,
            };
        }
    }

    Decision {
        verdict: Verdict::Allow(format!("Gate 2 allowed {} for WIH {}", req.tool_name, wih.wih_id)),
        paths: rel_paths,
    }
}

/// Same matching rule as `Leases::check_coverage`.
fn lease_matches(lease_path: &str, candidate: &str) -> bool {
    if let Some(prefix) = lease_path.strip_suffix("/**") {
        return candidate.starts_with(prefix);
    }
    if let Some(prefix) = lease_path.strip_suffix('*') {
        return candidate.starts_with(prefix);
    }
    candidate == lease_path || candidate.starts_with(&format!("{lease_path}/"))
}

/// A path and its symlink-resolved form (macOS `/tmp` → `/private/tmp`).
fn path_forms(path: &Path) -> Vec<PathBuf> {
    let mut forms = vec![shell::normalize(path)];
    let canon = canonical_lenient(path);
    if !forms.contains(&canon) {
        forms.push(canon);
    }
    forms
}

/// Canonicalize the longest existing ancestor and re-append the rest, so
/// not-yet-created files still resolve through symlinked parents.
fn canonical_lenient(path: &Path) -> PathBuf {
    let normalized = shell::normalize(path);
    let mut existing = normalized.clone();
    let mut tail = Vec::new();
    loop {
        if let Ok(canon) = std::fs::canonicalize(&existing) {
            let mut out = canon;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                existing = parent.to_path_buf();
            }
            _ => return normalized,
        }
    }
}

fn relative_to_root(path: &Path, root_forms: &[PathBuf]) -> Option<String> {
    for candidate in path_forms(path) {
        for root in root_forms {
            if let Ok(rel) = candidate.strip_prefix(root) {
                let rel = rel.to_string_lossy().to_string();
                return Some(if rel.is_empty() { ".".to_string() } else { rel });
            }
        }
    }
    None
}

/// Which vendor hook dialect a `HarnessGate::Hook` harness speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFlavor {
    Claude,
    Codex,
    Qwen,
}

impl HookFlavor {
    pub fn of(harness: &str) -> Option<Self> {
        match shell::basename(harness.trim()).to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "qwen" | "qwen-code" => Some(Self::Qwen),
            _ => None,
        }
    }
    /// Subcommand of `allternit-commrails hook`. All three run the same
    /// decision path (`decide`); the names only label the harness.
    pub fn subcommand(self) -> &'static str {
        match self {
            Self::Claude => "claude-pretool",
            Self::Codex => "codex-pretool",
            Self::Qwen => "qwen-pretool",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex",
            Self::Qwen => "qwen",
        }
    }
}

/// Claude Code PreToolUse stdout for a verdict. Allow prints nothing so the
/// session's permission rules still apply; deny blocks the call.
pub fn claude_hook_output(verdict: &Verdict) -> Option<String> {
    match verdict {
        Verdict::Allow(_) => None,
        Verdict::Deny(reason) => Some(
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": format!("Allternit spawn gate: {reason}"),
                }
            })
            .to_string(),
        ),
    }
}

/// Ledger event for a hook decision.
pub fn decision_event(req: &HookRequest, harness: &str, wih_id: Option<&str>, decision: &Decision) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "spawn-gate".to_string(),
        },
        scope: wih_id.map(|w| EventScope {
            wih_id: Some(w.to_string()),
            ..Default::default()
        }),
        r#type: HOOK_EVENT.to_string(),
        payload: json!({
            "wih_id": wih_id,
            "harness": harness,
            "harness_session_id": req.session_id,
            "tool": req.tool_name,
            "decision": if decision.verdict.is_deny() { "deny" } else { "allow" },
            "reason": decision.verdict.reason(),
            "paths": decision.paths,
            "command": req.command(),
        }),
        provenance: None,
    }
}

/// Ledger event for a refused spawn.
pub fn spawn_refused_event(harness: &str, wih_id: &str, reason: &str) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "spawn-gate".to_string(),
        },
        scope: Some(EventScope {
            wih_id: Some(wih_id.to_string()),
            ..Default::default()
        }),
        r#type: SPAWN_REFUSED_EVENT.to_string(),
        payload: json!({ "wih_id": wih_id, "harness": harness, "reason": reason }),
        provenance: None,
    }
}

/// Tools a headless Claude Code run needs without a prompt. The session runs
/// in `bypassPermissions` so nothing blocks on a CLI-side prompt; the
/// PreToolUse hook still sees (and can deny) every call.
pub const CLAUDE_ALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "Edit",
    "Write",
    "MultiEdit",
    "NotebookEdit",
    "Read",
    "Glob",
    "Grep",
    "LS",
    "WebFetch",
    "WebSearch",
    "TodoWrite",
    "Task",
    "Agent",
];

/// Single-quote a word for `/bin/sh`.
pub fn sh_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Where a spawned session's hook points.
#[derive(Debug, Clone, Copy)]
pub struct HookTarget<'a> {
    /// The `allternit-commrails` binary the hook runs.
    pub commrails_bin: &'a Path,
    /// CommRails root holding `.allternit/` (ledger, leases).
    pub root: &'a Path,
    /// Directory the harness works in; write paths are made relative to it
    /// for lease matching. `None` means the root itself.
    pub workspace: Option<&'a Path>,
    /// WIH the session is bound to.
    pub wih_id: Option<&'a str>,
}

/// The hook command line Claude Code runs for every tool call.
pub fn claude_hook_command(target: HookTarget<'_>) -> String {
    hook_command(HookFlavor::Claude, target)
}

/// The hook command line for any hooked harness.
pub fn hook_command(flavor: HookFlavor, target: HookTarget<'_>) -> String {
    let mut cmd = format!(
        "{} --root {} hook {} --harness {}",
        sh_quote(&target.commrails_bin.to_string_lossy()),
        sh_quote(&target.root.to_string_lossy()),
        flavor.subcommand(),
        flavor.label()
    );
    if let Some(ws) = target.workspace {
        cmd.push_str(" --workspace ");
        cmd.push_str(&sh_quote(&ws.to_string_lossy()));
    }
    if let Some(wih) = target.wih_id {
        cmd.push_str(" --wih ");
        cmd.push_str(&sh_quote(wih));
    }
    cmd
}

/// Session-scoped Claude Code settings (`claude --settings <file>`).
pub fn claude_settings(target: HookTarget<'_>) -> Value {
    json!({
        "permissions": {
            "defaultMode": "bypassPermissions",
            "allow": CLAUDE_ALLOWED_TOOLS,
        },
        "hooks": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": claude_hook_command(target),
                    "timeout": 30,
                }]
            }]
        }
    })
}

/// Session-scoped qwen settings (`QWEN_CODE_SYSTEM_SETTINGS_PATH=<file>`).
/// Same PreToolUse schema and `permissionDecision` output as Claude Code;
/// the user's `~/.qwen/settings.json` is never touched.
pub fn qwen_settings(target: HookTarget<'_>) -> Value {
    json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": hook_command(HookFlavor::Qwen, target),
                    "timeout": 30,
                }]
            }]
        }
    })
}

/// TOML basic string.
fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The per-spawn `-c` override registering codex's PreToolUse hook.
pub fn codex_hook_override(target: HookTarget<'_>) -> String {
    format!(
        "hooks.PreToolUse=[{{matcher=\"*\",hooks=[{{type=\"command\",command={},timeout=30}}]}}]",
        toml_str(&hook_command(HookFlavor::Codex, target))
    )
}

/// Session settings file a hooked harness reads, if it uses one:
/// `(file suffix, contents)`. Codex takes its hook via `-c`, no file.
pub fn hook_settings_file(harness: &str, target: HookTarget<'_>) -> Option<(&'static str, Value)> {
    match HookFlavor::of(harness)? {
        HookFlavor::Claude => Some(("claude-settings.json", claude_settings(target))),
        HookFlavor::Qwen => Some(("qwen-settings.json", qwen_settings(target))),
        HookFlavor::Codex => None,
    }
}

/// Locate the `allternit-commrails` binary a hook should run:
/// `$ALLTERNIT_COMMRAILS_BIN`, then a sibling of the current executable,
/// then `PATH`. `None` means no hook can be installed — callers must not fall
/// back to an unhooked bypass run.
pub fn find_commrails_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ALLTERNIT_COMMRAILS_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["allternit-commrails", "allternit-rails"] {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("allternit-commrails"))
        .find(|c| c.is_file())
}

/// Argv rewrite for an orchestrator spawn so the harness is launched gated.
/// Returns the new argv; `settings_path` must already contain
/// [`claude_settings`] for Claude.
pub fn gate_argv(cmd: &[String], settings_path: Option<&Path>) -> Vec<String> {
    gate_spawn(cmd, settings_path, None).argv
}

/// A gated launch: rewritten argv plus env the harness needs to find its
/// per-spawn gate config.
#[derive(Debug, Clone, PartialEq)]
pub struct GatedSpawn {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// [`gate_argv`] for every hooked harness. `settings_path` holds the session
/// settings ([`hook_settings_file`]); `target` is needed for codex, whose
/// hook rides `-c`.
pub fn gate_spawn(cmd: &[String], settings_path: Option<&Path>, target: Option<HookTarget<'_>>) -> GatedSpawn {
    let Some(first) = cmd.first() else {
        return GatedSpawn { argv: Vec::new(), env: Vec::new() };
    };
    match HookFlavor::of(first) {
        Some(HookFlavor::Codex) => return gate_codex(cmd, target),
        Some(HookFlavor::Qwen) => return gate_qwen(cmd, settings_path),
        _ => {}
    }
    let argv = gate_argv_claude_or_other(cmd, settings_path);
    GatedSpawn { argv, env: Vec::new() }
}

/// codex: full auto-approve/no codex sandbox, plus our PreToolUse hook, all
/// per spawn (`-c` + the hook-trust flag; `~/.codex` is never written).
/// Global options go right after the binary so they precede `exec`,
/// `exec resume`, or no subcommand.
fn gate_codex(cmd: &[String], target: Option<HookTarget<'_>>) -> GatedSpawn {
    let mut rest = Vec::with_capacity(cmd.len());
    let mut skip_next = false;
    for (i, w) in cmd.iter().enumerate().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        let overrides_policy = |v: &str| {
            v.starts_with("sandbox_mode") || v.starts_with("approval_policy") || v.starts_with("hooks.PreToolUse")
        };
        if (w == "-c" || w == "--config") && cmd.get(i + 1).map(|v| overrides_policy(v)).unwrap_or(false) {
            skip_next = true;
            continue;
        }
        if w == "--dangerously-bypass-approvals-and-sandbox"
            || w == "--yolo"
            || w == "--dangerously-bypass-hook-trust"
            || w.starts_with("--sandbox=")
        {
            continue;
        }
        if w == "--sandbox" || w == "-s" {
            skip_next = true;
            continue;
        }
        rest.push(w.clone());
    }
    let mut argv = vec![cmd[0].clone()];
    for config in ["sandbox_mode=\"danger-full-access\"", "approval_policy=\"never\""] {
        argv.push("-c".to_string());
        argv.push(config.to_string());
    }
    if let Some(target) = target {
        argv.push("-c".to_string());
        argv.push(codex_hook_override(target));
        argv.push("--dangerously-bypass-hook-trust".to_string());
    }
    argv.extend(rest);
    GatedSpawn { argv, env: Vec::new() }
}

/// qwen: keep `--yolo`; the hook comes from a system-settings file.
fn gate_qwen(cmd: &[String], settings_path: Option<&Path>) -> GatedSpawn {
    let mut argv = vec![cmd[0].clone()];
    let mut skip_next = false;
    for w in &cmd[1..] {
        if skip_next {
            skip_next = false;
            continue;
        }
        match w.as_str() {
            "--yolo" | "-y" => continue,
            "--approval-mode" => {
                skip_next = true;
                continue;
            }
            _ if w.starts_with("--approval-mode=") => continue,
            _ => argv.push(w.clone()),
        }
    }
    argv.push("--yolo".to_string());
    let env = settings_path
        .map(|p| vec![("QWEN_CODE_SYSTEM_SETTINGS_PATH".to_string(), p.to_string_lossy().to_string())])
        .unwrap_or_default();
    GatedSpawn { argv, env }
}

fn gate_argv_claude_or_other(cmd: &[String], settings_path: Option<&Path>) -> Vec<String> {
    let Some(first) = cmd.first() else { return Vec::new() };
    match harness_gate(first) {
        HarnessGate::Hook => {
            let mut out = vec![first.clone()];
            let mut skip_next = false;
            for w in &cmd[1..] {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                match w.as_str() {
                    "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions" => continue,
                    "--permission-mode" => {
                        skip_next = true;
                        continue;
                    }
                    _ if w.starts_with("--permission-mode=") => continue,
                    _ => out.push(w.clone()),
                }
            }
            // Auto-approve on the CLI's side; the PreToolUse hook in
            // --settings still runs in bypass mode and its deny wins.
            out.push("--permission-mode".to_string());
            out.push("bypassPermissions".to_string());
            if let Some(settings) = settings_path {
                out.push("--settings".to_string());
                out.push(settings.to_string_lossy().to_string());
            }
            out
        }
        HarnessGate::Sandbox => {
            // Caller-chosen sandbox/approval flags are replaced by one
            // consistent auto-approve setting.
            let mut out = Vec::with_capacity(cmd.len() + 4);
            let mut skip_next = false;
            for (i, w) in cmd.iter().enumerate() {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                let overrides_policy = |v: &str| v.starts_with("sandbox_mode") || v.starts_with("approval_policy");
                if (w == "-c" || w == "--config") && cmd.get(i + 1).map(|v| overrides_policy(v)).unwrap_or(false) {
                    skip_next = true;
                    continue;
                }
                if w == "--dangerously-bypass-approvals-and-sandbox" || w == "--yolo" || w.starts_with("--sandbox=") {
                    continue;
                }
                if w == "--sandbox" || w == "-s" {
                    skip_next = true;
                    continue;
                }
                out.push(w.clone());
            }
            // Auto-approve with no codex-side sandbox (the yolo equivalent).
            // `-c` rides every codex subcommand (`codex exec resume` has no
            // `--sandbox` or bypass flag). Confinement comes from Allternit's
            // execution environment, not from codex.
            for config in [
                "sandbox_mode=\"danger-full-access\"",
                "approval_policy=\"never\"",
            ] {
                out.push("-c".to_string());
                out.push(config.to_string());
            }
            out
        }
        HarnessGate::Acp | HarnessGate::Ungated => cmd.to_vec(),
    }
}

#[cfg(test)]
mod tests;
