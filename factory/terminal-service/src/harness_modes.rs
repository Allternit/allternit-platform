//! Per-CLI permission-mode flags and start argv for "Start anywhere": the one table every
//! caller reads (allternit-api, allternit-node, [`crate::start`], and the Factory harness table,
//! which re-exports it). Pure data and functions, no engine types, so a crate that only links
//! terminal-service (allternit-node) uses the same flags as the engine.
//!
//! Harness ids match the Factory harness table (`allternit_factory_engine::agents::harness_table`);
//! a test there checks every harness row is listed in [`START_HARNESSES`].

/// The app's permission mode ids, lowest to highest (`plan < default < auto < bypass`).
pub const START_MODES: [&str; 4] = ["auto", "default", "plan", "bypass"];

/// Harness ids that can start a new session. Each one's program name is its id.
pub const START_HARNESSES: [&str; 8] = ["claude", "codex", "kimi", "grok", "agy", "gizzi", "opencode", "qwen"];

/// The harness id a runtime registry id names (`codex-cli` -> `codex`, `antigravity` -> `agy`),
/// lowercased, whether or not the harness is known.
pub fn runtime_harness_id(runtime_id: &str) -> String {
    let id = runtime_id.trim().to_ascii_lowercase();
    let id = id.strip_suffix("-cli").unwrap_or(id.as_str());
    match id {
        "antigravity" => "agy",
        "gizzi-code" => "gizzi",
        "kimi-code" => "kimi",
        other => other,
    }
    .to_string()
}

/// The known harness id for a runtime registry id (`claude`, `codex-cli`, `antigravity`, …).
pub fn harness_id_for_runtime(runtime_id: &str) -> Option<&'static str> {
    let id = runtime_harness_id(runtime_id);
    START_HARNESSES.iter().copied().find(|h| *h == id)
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// The launch flags that start a new session of this runtime in an app permission mode
/// (`auto|default|plan|bypass`). `None`: the CLI has no flag for that mode (or the runtime is
/// unknown), so the mode is not offered for it. Read from each CLI's `--help` on 2026-11-27:
///
/// | harness | auto | default | plan | bypass |
/// |---|---|---|---|---|
/// | claude 2.1.332 | `--permission-mode acceptEdits` | `--permission-mode manual` | `--permission-mode plan` | `--permission-mode bypassPermissions` |
/// | codex 0.171.0 | `-s workspace-write -a on-request` | `-s read-only -a on-request` | none | `--dangerously-bypass-approvals-and-sandbox` |
/// | grok | `--permission-mode acceptEdits` | `--permission-mode default` | `--permission-mode plan` | `--permission-mode bypassPermissions` |
/// | kimi | `--yolo` (Ask When Needed) | none | `--plan` | `--auto` (Never Ask) |
/// | agy | `--mode accept-edits` | none | `--mode plan` | `--dangerously-skip-permissions` |
/// | qwen | none | none | none | `--yolo` |
/// | opencode | none | none | none | `--auto` |
///
/// The app's `auto` is claude's `acceptEdits` (same mapping as the chat send path). claude
/// 2.1.332 lists `manual` where older builds said `default`. "none" in the default column: the
/// CLI has no flag that forces asking, and starting with no flag would use its own config, which
/// may be higher. gizzi runs as the engine (runtime `allternit`), not through this table.
pub fn mode_flags(runtime_id: &str, mode: &str) -> Option<Vec<String>> {
    let h = harness_id_for_runtime(runtime_id)?;
    let flags: &[&str] = match (h, mode) {
        ("claude", "auto") | ("grok", "auto") => &["--permission-mode", "acceptEdits"],
        ("claude", "default") => &["--permission-mode", "manual"],
        ("grok", "default") => &["--permission-mode", "default"],
        ("claude", "plan") | ("grok", "plan") => &["--permission-mode", "plan"],
        ("claude", "bypass") | ("grok", "bypass") => &["--permission-mode", "bypassPermissions"],
        ("codex", "auto") => &["--sandbox", "workspace-write", "--ask-for-approval", "on-request"],
        ("codex", "default") => &["--sandbox", "read-only", "--ask-for-approval", "on-request"],
        ("codex", "bypass") => &["--dangerously-bypass-approvals-and-sandbox"],
        ("kimi", "auto") => &["--yolo"],
        ("kimi", "plan") => &["--plan"],
        ("kimi", "bypass") => &["--auto"],
        ("agy", "auto") => &["--mode", "accept-edits"],
        ("agy", "plan") => &["--mode", "plan"],
        ("agy", "bypass") => &["--dangerously-skip-permissions"],
        ("qwen", "bypass") => &["--yolo"],
        ("opencode", "bypass") => &["--auto"],
        _ => return None,
    };
    Some(args(flags))
}

/// How a CLI takes a first prompt that starts an interactive session. `None`: it takes none (or
/// the prompt would read as a flag), so the caller types it into the terminal after start.
/// claude, codex and grok take it as the positional `[prompt]`; agy as `--prompt-interactive`,
/// qwen as `-i/--prompt-interactive`, opencode as `--prompt`. kimi's `-p` prints and exits.
pub fn prompt_args(runtime_id: &str, prompt: &str) -> Option<Vec<String>> {
    let h = harness_id_for_runtime(runtime_id)?;
    if prompt.trim().is_empty() {
        return None;
    }
    match h {
        // A leading `-` would parse as a flag.
        "claude" | "codex" | "grok" => (!prompt.starts_with('-')).then(|| vec![prompt.to_string()]),
        "agy" => Some(vec!["--prompt-interactive".to_string(), prompt.to_string()]),
        "qwen" => Some(vec!["--prompt-interactive".to_string(), prompt.to_string()]),
        "opencode" => Some(vec!["--prompt".to_string(), prompt.to_string()]),
        _ => None,
    }
}

/// The argv that starts a new session of `runtime_id` in `cwd`'s terminal: the program, the mode
/// flags (none when `mode` is `None`: the CLI's own default), then the prompt when the CLI takes
/// one. The bool is true when the prompt is in the argv; false means type it in after start.
/// `None`: unknown runtime, or a mode the CLI has no flag for.
pub fn start_argv(runtime_id: &str, mode: Option<&str>, prompt: Option<&str>) -> Option<(Vec<String>, bool)> {
    let h = harness_id_for_runtime(runtime_id)?;
    let mut argv = vec![h.to_string()];
    if let Some(mode) = mode {
        argv.extend(mode_flags(h, mode)?);
    }
    let in_argv = match prompt.and_then(|p| prompt_args(h, p)) {
        Some(p) => {
            argv.extend(p);
            true
        }
        None => false,
    };
    Some((argv, in_argv))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn harness_modes_mode_flags_and_start_argv() {
        assert_eq!(mode_flags("claude", "auto"), Some(v(&["--permission-mode", "acceptEdits"])));
        assert_eq!(mode_flags("claude", "default"), Some(v(&["--permission-mode", "manual"])));
        assert_eq!(mode_flags("claude", "plan"), Some(v(&["--permission-mode", "plan"])));
        assert_eq!(mode_flags("claude-cli", "bypass"), Some(v(&["--permission-mode", "bypassPermissions"])));
        assert_eq!(mode_flags("codex", "auto"), Some(v(&["--sandbox", "workspace-write", "--ask-for-approval", "on-request"])));
        assert_eq!(mode_flags("codex", "default"), Some(v(&["--sandbox", "read-only", "--ask-for-approval", "on-request"])));
        assert_eq!(mode_flags("codex-cli", "bypass"), Some(v(&["--dangerously-bypass-approvals-and-sandbox"])));
        // No flag for that mode: not offered.
        assert_eq!(mode_flags("codex", "plan"), None);
        assert_eq!(mode_flags("kimi", "default"), None);
        assert_eq!(mode_flags("antigravity", "plan"), Some(v(&["--mode", "plan"])));
        assert_eq!(mode_flags("claude", "acceptEdits"), None);
        assert_eq!(mode_flags("allternit", "auto"), None);

        assert_eq!(start_argv("claude", Some("auto"), Some("fix it")), Some((v(&["claude", "--permission-mode", "acceptEdits", "fix it"]), true)));
        assert_eq!(start_argv("codex", None, Some("fix it")), Some((v(&["codex", "fix it"]), true)));
        // A prompt that reads as a flag is typed in after start.
        assert_eq!(start_argv("claude", None, Some("--help me")), Some((v(&["claude"]), false)));
        assert_eq!(start_argv("kimi", Some("plan"), Some("fix it")), Some((v(&["kimi", "--plan"]), false)));
        assert_eq!(start_argv("opencode", Some("bypass"), Some("go")), Some((v(&["opencode", "--auto", "--prompt", "go"]), true)));
        assert_eq!(start_argv("codex", Some("plan"), None), None);
        assert_eq!(start_argv("nope", None, None), None);
    }

    #[test]
    fn harness_modes_runtime_ids() {
        assert_eq!(harness_id_for_runtime(" Codex-CLI "), Some("codex"));
        assert_eq!(harness_id_for_runtime("gizzi-code"), Some("gizzi"));
        assert_eq!(harness_id_for_runtime("kimi-code"), Some("kimi"));
        assert_eq!(harness_id_for_runtime("allternit"), None);
        assert_eq!(runtime_harness_id("allternit"), "allternit");
    }
}
