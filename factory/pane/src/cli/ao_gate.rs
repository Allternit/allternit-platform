//! Spawn gate (audit S1) for `ao spawn` / `ao recover --apply`.
//!
//! Byte-for-byte mirror of `tools/agent-orchestrator/scripts/ao-spawn-gate`,
//! the bash gate every `ao-spawn` launch line goes through. Same gated runner
//! line, same stderr, same refusal exit, same `spawn-gate.log` line. Parity is
//! checked black-box by `tests/ao_parity/gate_parity.sh` (rewrite table) and
//! `tests/ao_parity/run.sh` (full spawn).
//!
//! Policy is allternit-commrails `hook` (PR #965):
//! - claude / claude-code (`hook`): `--dangerously-skip-permissions` becomes
//!   `--permission-mode acceptEdits --settings <file>`; the settings file comes
//!   from `allternit-commrails hook claude-settings` and carries the PreToolUse
//!   hook (hard floor + Gate 2). No bypass flag: the same flags are inserted
//!   after the harness word. No commrails binary: the spawn is refused.
//! - codex (`sandbox`): `--dangerously-bypass-approvals-and-sandbox` becomes the
//!   workspace-write sandbox flags; `danger-full-access` becomes
//!   `workspace-write`.
//! - everything else (`ungated`): unchanged, labeled in the spawn log.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CLAUDE_BYPASS: &str = "--dangerously-skip-permissions";
const CLAUDE_ALLOW_BYPASS: &str = "--allow-dangerously-skip-permissions";
const CODEX_BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";
pub(super) const CODEX_FLAGS: &str = "-c 'sandbox_mode=\"workspace-write\"' -c 'approval_policy=\"never\"' -c sandbox_workspace_write.network_access=true";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GateClass {
    Hook,
    Sandbox,
    Ungated,
}

impl GateClass {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Sandbox => "sandbox",
            Self::Ungated => "ungated",
        }
    }
}

/// Result of an admitted spawn.
#[derive(Debug, Clone)]
pub(super) struct Gated {
    pub line: String,
    pub class: GateClass,
}

/// bash `[[:space:]]` in the C locale.
fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0B' | '\x0C' | '\r')
}

fn sh_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// (prefix through the harness word, rest, lowercased harness basename).
fn split_harness(line: &str) -> (&str, &str, String) {
    let mut pos = 0;
    let bytes_len = line.len();
    loop {
        // leading whitespace
        let ws_len = line[pos..].find(|c: char| !is_ws(c)).unwrap_or(bytes_len - pos);
        pos += ws_len;
        let word_len = line[pos..].find(is_ws).unwrap_or(bytes_len - pos);
        if word_len == 0 {
            return (&line[..pos], &line[pos..], String::new());
        }
        let word = &line[pos..pos + word_len];
        pos += word_len;
        if matches!(word, "env" | "exec" | "command" | "nohup") || is_assignment(word) {
            continue;
        }
        let base = word.rsplit('/').next().unwrap_or(word);
        return (&line[..pos], &line[pos..], base.to_ascii_lowercase());
    }
}

/// `^[A-Za-z_][A-Za-z0-9_]*=`
fn is_assignment(word: &str) -> bool {
    let Some(eq) = word.find('=') else { return false };
    let name = &word[..eq];
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whole-word replace: preceded by start/whitespace, followed by
/// end/whitespace or one of `;&|)`. Returns (text, replacements).
fn replace_word(s: &str, word: &str, repl: &str) -> (String, usize) {
    let mut out = String::with_capacity(s.len());
    let mut last_end = 0;
    let mut n = 0;
    for (i, _) in s.match_indices(word) {
        let pre_ok = s[..i].chars().last().map_or(true, is_ws);
        let post_ok = s[i + word.len()..]
            .chars()
            .next()
            .map_or(true, |c| is_ws(c) || matches!(c, ';' | '&' | '|' | ')'));
        out.push_str(&s[last_end..i]);
        if pre_ok && post_ok {
            out.push_str(repl);
            n += 1;
        } else {
            out.push_str(word);
        }
        last_end = i + word.len();
    }
    out.push_str(&s[last_end..]);
    (out, n)
}

pub(super) fn classify(harness: &str) -> GateClass {
    match harness {
        "claude" | "claude-code" => GateClass::Hook,
        "codex" => GateClass::Sandbox,
        _ => GateClass::Ungated,
    }
}

/// Harness basename (lowercased) of a launch line.
pub(super) fn harness_of(line: &str) -> String {
    split_harness(line).2
}

fn insert_after_harness(line: &str, flags: &str) -> String {
    let (prefix, rest, _) = split_harness(line);
    format!("{prefix} {flags}{rest}")
}

/// Pure text rewrite. Returns (gated line, notice).
pub(super) fn rewrite(line: &str, class: GateClass, settings: &str) -> (String, Option<String>) {
    match class {
        GateClass::Hook => {
            let flags = format!("--permission-mode acceptEdits --settings {}", sh_quote(settings));
            let (line, main) = replace_word(line, CLAUDE_BYPASS, &flags);
            let (mut line, allow) = replace_word(&line, CLAUDE_ALLOW_BYPASS, "");
            let mut n = main + allow;
            if line.contains("bypassPermissions") {
                line = line.replace("bypassPermissions", "acceptEdits");
                n += 1;
            }
            if main == 0 && !line.contains(&flags) {
                line = insert_after_harness(&line, &flags);
            }
            let notice = (n > 0).then(|| {
                format!(
                    "spawn gate: rewrote claude permission bypass to --permission-mode acceptEdits --settings {settings} (PreToolUse hook: hard floor + Gate 2)"
                )
            });
            (line, notice)
        }
        GateClass::Sandbox => {
            let (mut line, main) = replace_word(line, CODEX_BYPASS, CODEX_FLAGS);
            let mut n = main;
            if line.contains("danger-full-access") {
                line = line.replace("danger-full-access", "workspace-write");
                n += 1;
            }
            if main == 0 && !line.contains(CODEX_FLAGS) {
                line = insert_after_harness(&line, CODEX_FLAGS);
            }
            let notice = (n > 0).then(|| format!("spawn gate: rewrote codex sandbox bypass to {CODEX_FLAGS}"));
            (line, notice)
        }
        GateClass::Ungated => (line.to_string(), None),
    }
}

/// Argv the gate binary needs before the hook's own argv:
/// `allternit-factory internal hook --root … spawn-check|claude-settings …`.
const GATE_ARGV_PREFIX: [&str; 2] = ["internal", "hook"];

/// The engine binary that carries the gate: this process when it is
/// `allternit-factory` (the normal case), else `$ALLTERNIT_FACTORY_BIN` (or the
/// deprecated `$ALLTERNIT_COMMRAILS_BIN`), else an `allternit-factory` next to
/// this executable. Never one found on `PATH` — the same rule as the engine's
/// `hook::find_commrails_bin`, which this delegates to.
fn commrails_bin() -> Option<PathBuf> {
    allternit_factory_engine::hook::find_commrails_bin()
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Admit, prepare and rewrite one spawn. `Err(())` means the refusal was
/// already printed on stderr (exit 1, like the script's `ao_gate || exit 1`).
pub(super) fn gate(session: &str, workdir: &str, line: &str, logs_dir: &Path, ao_home: &Path) -> Result<Gated, ()> {
    let harness = harness_of(line);
    let class = classify(&harness);
    let root = env_nonempty("ALLTERNIT_COMMRAILS_ROOT").unwrap_or_else(|| ao_home.display().to_string());
    let settings = logs_dir.join(format!("{session}.claude-settings.json"));
    let wih = env_nonempty("ALLTERNIT_COMMRAILS_WIH");
    let shown = if harness.is_empty() { "the agent" } else { harness.as_str() };

    let mut bin = None;
    if class == GateClass::Hook || wih.is_some() {
        match commrails_bin() {
            Some(found) => bin = Some(found),
            None => {
                eprintln!(
                    "error: spawn gate: allternit-factory not found (set ALLTERNIT_COMMRAILS_BIN); refusing to run {shown} without its gate"
                );
                return Err(());
            }
        }
    }
    if class != GateClass::Hook {
        if let (Some(wih), Some(bin)) = (wih.as_deref(), bin.as_ref()) {
            let harness_arg = if harness.is_empty() { "-" } else { harness.as_str() };
            let output = Command::new(bin)
                .args(GATE_ARGV_PREFIX)
                .arg("--root")
                .arg(&root)
                .args(["spawn-check", "--harness", harness_arg, "--wih", wih])
                .stdin(Stdio::null())
                .output();
            let refused = match &output {
                Ok(out) => (!out.status.success()).then(|| String::from_utf8_lossy(&out.stderr).into_owned()),
                Err(err) => Some(format!("{err}")),
            };
            if let Some(reason) = refused {
                // $(... 2>&1 >/dev/null) strips trailing newlines.
                eprintln!("error: spawn gate: {}", reason.trim_end_matches('\n'));
                return Err(());
            }
        }
    }
    if class == GateClass::Hook {
        let bin = bin.as_ref().expect("hook class resolved the binary");
        let _ = std::fs::create_dir_all(logs_dir);
        let ok = Command::new(bin)
            .args(GATE_ARGV_PREFIX)
            .arg("--root")
            .arg(&root)
            .args(["claude-settings", "--workspace"])
            .arg(workdir)
            .arg("--out")
            .arg(&settings)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!(
                "error: spawn gate: {} internal hook claude-settings failed; refusing to run {harness} without its gate",
                bin.display()
            );
            return Err(());
        }
    }

    let (gated, notice) = rewrite(line, class, &settings.display().to_string());
    if let Some(notice) = notice {
        eprintln!("{notice}");
    }
    let _ = std::fs::create_dir_all(logs_dir);
    let entry = format!(
        "{} {session} gate={} harness={}\n",
        super::ao::timestamp_now(),
        class.as_str(),
        if harness.is_empty() { "-" } else { harness.as_str() }
    );
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("spawn-gate.log"))
    {
        let _ = f.write_all(entry.as_bytes());
    }
    Ok(Gated { line: gated, class })
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "/h/.agent-orchestrator/logs/ao-x.claude-settings.json";

    fn rw(line: &str) -> (String, Option<String>) {
        rewrite(line, classify(&harness_of(line)), S)
    }

    #[test]
    fn claude_bypass_is_rewritten_in_place() {
        let (line, notice) = rw("claude -p 'do it' --dangerously-skip-permissions; touch s");
        assert_eq!(
            line,
            format!("claude -p 'do it' --permission-mode acceptEdits --settings '{S}'; touch s")
        );
        assert!(notice.unwrap().starts_with("spawn gate: rewrote claude permission bypass"));
    }

    #[test]
    fn claude_without_bypass_gets_flags_after_harness() {
        let (line, notice) = rw("FOO=1 env /opt/bin/Claude -p hi");
        assert_eq!(line, format!("FOO=1 env /opt/bin/Claude --permission-mode acceptEdits --settings '{S}' -p hi"));
        assert!(notice.is_none());
    }

    #[test]
    fn claude_rewrite_is_idempotent() {
        let (once, _) = rw("claude --dangerously-skip-permissions");
        let (twice, notice) = rw(&once);
        assert_eq!(once, twice);
        assert!(notice.is_none());
    }

    #[test]
    fn claude_bypass_mode_and_allow_flag_removed() {
        let (line, notice) = rw("claude --permission-mode bypassPermissions --allow-dangerously-skip-permissions");
        assert!(!line.contains("bypassPermissions") && !line.contains("dangerously"), "{line}");
        assert!(notice.is_some());
    }

    #[test]
    fn whole_word_only() {
        let (line, _) = rw("claude --x--dangerously-skip-permissions --dangerously-skip-permissions|cat");
        assert_eq!(
            line,
            format!("claude --x--dangerously-skip-permissions --permission-mode acceptEdits --settings '{S}'|cat")
        );
    }

    #[test]
    fn codex_bypass_becomes_sandbox() {
        let (line, notice) = rw("codex exec 'x' --dangerously-bypass-approvals-and-sandbox");
        assert_eq!(line, format!("codex exec 'x' {CODEX_FLAGS}"));
        assert!(notice.is_some());
        let (line, _) = rw("codex --sandbox danger-full-access");
        assert_eq!(line, format!("codex {CODEX_FLAGS} --sandbox workspace-write"));
        assert!(!line.contains("danger"));
    }

    #[test]
    fn other_harnesses_unchanged() {
        for l in ["kimi --yolo", "agy --dangerously-skip-permissions", "sh /tmp/agent.sh", ""] {
            let (line, notice) = rw(l);
            assert_eq!(line, l);
            assert!(notice.is_none());
            assert_eq!(classify(&harness_of(l)), GateClass::Ungated);
        }
    }

    #[test]
    fn settings_path_is_shell_quoted() {
        let (line, _) = rewrite("claude", GateClass::Hook, "/a/it's.json");
        assert_eq!(line, "claude --permission-mode acceptEdits --settings '/a/it'\\''s.json'");
    }
}
