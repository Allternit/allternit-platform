//! Read-only consult profiles for the observer.
//!
//! The observer only ever runs a consult command through one of these
//! profiles. Known agent CLIs get their read-only flags appended; any other
//! command is refused unless the operator attests it is read-only
//! (`read_only_attested: true` / `ALLTERNIT_OBSERVER_READ_ONLY_ATTESTED=1`,
//! e.g. a stub or an API-only script with no tools).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Tools the claude profile keeps (read-only built-ins).
pub const CLAUDE_READ_TOOLS: &str = "Read,Grep,Glob";
/// Tools the claude profile denies explicitly (belt and braces with
/// `--tools` + `--permission-mode plan`).
pub const CLAUDE_DENIED_TOOLS: &str = "Bash,Edit,Write,NotebookEdit,MultiEdit";

/// How the prompt reaches the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptVia {
    Stdin,
    /// Appended as the value of this flag (kimi `-p <prompt>`).
    Flag(&'static str),
}

/// A fully resolved read-only invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyCommand {
    /// `claude` | `kimi` | `codex` | `attested`.
    pub profile: &'static str,
    pub program: String,
    pub args: Vec<String>,
    pub prompt_via: PromptVia,
}

/// Split `--model X` / `-m X` out of the user's extra args; anything else is
/// refused for a known CLI, because the observer cannot vouch for flags it
/// does not understand (they could re-enable writes).
fn model_only(extra: &[&str], program: &str) -> Result<Option<String>> {
    let mut model = None;
    let mut it = extra.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--model" | "-m" => {
                model = Some(
                    it.next()
                        .ok_or_else(|| anyhow!("{arg} needs a value"))?
                        .to_string(),
                )
            }
            other => bail!(
                "observer refuses extra flag {other:?} for {program}: only --model is allowed \
                 so the read-only profile cannot be overridden"
            ),
        }
    }
    Ok(model)
}

/// Resolve `consult_cmd` into a read-only invocation, or refuse.
pub fn resolve_read_only(consult_cmd: &str, attested: bool) -> Result<ReadOnlyCommand> {
    let parts: Vec<&str> = consult_cmd.split_whitespace().collect();
    let Some(first) = parts.first() else {
        bail!("observer consult command is empty");
    };
    let base = Path::new(first)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(first);
    match base {
        "claude" => {
            let model = model_only(&parts[1..], base)?;
            let mut args = vec![
                "-p".to_string(),
                "--permission-mode".to_string(),
                "plan".to_string(),
                "--tools".to_string(),
                CLAUDE_READ_TOOLS.to_string(),
                "--disallowedTools".to_string(),
                CLAUDE_DENIED_TOOLS.to_string(),
                "--strict-mcp-config".to_string(),
            ];
            if let Some(m) = model {
                args.extend(["--model".to_string(), m]);
            }
            Ok(ReadOnlyCommand {
                profile: "claude",
                program: first.to_string(),
                args,
                prompt_via: PromptVia::Stdin,
            })
        }
        "kimi" => {
            let model = model_only(&parts[1..], base)?;
            let mut args = vec!["--plan".to_string()];
            if let Some(m) = model {
                args.extend(["--model".to_string(), m]);
            }
            Ok(ReadOnlyCommand {
                profile: "kimi",
                program: first.to_string(),
                args,
                prompt_via: PromptVia::Flag("-p"),
            })
        }
        "codex" => {
            let model = model_only(&parts[1..], base)?;
            let mut args = vec![
                "exec".to_string(),
                "--sandbox".to_string(),
                "read-only".to_string(),
                "--skip-git-repo-check".to_string(),
                "--ephemeral".to_string(),
            ];
            if let Some(m) = model {
                args.extend(["--model".to_string(), m]);
            }
            args.push("-".to_string());
            Ok(ReadOnlyCommand {
                profile: "codex",
                program: first.to_string(),
                args,
                prompt_via: PromptVia::Stdin,
            })
        }
        _ if attested => Ok(ReadOnlyCommand {
            profile: "attested",
            program: "bash".to_string(),
            args: vec!["-c".to_string(), consult_cmd.to_string()],
            prompt_via: PromptVia::Stdin,
        }),
        other => bail!(
            "observer has no read-only profile for {other:?}; use claude, kimi or codex, or set \
             consult_cmd to this command with read_only_attested: true in \
             .allternit/rails/observer.json if it has no write tools"
        ),
    }
}

impl ReadOnlyCommand {
    /// Run with `prompt`; returns stdout. Non-zero exit or empty output is an
    /// error. The child is killed when `timeout` elapses.
    pub async fn run(&self, cwd: &Path, prompt: &str, timeout: Duration) -> Result<String> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir(cwd)
            .env("ALLTERNIT_OBSERVER", "1")
            .env("ALLTERNIT_OBSERVER_READ_ONLY", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        match &self.prompt_via {
            PromptVia::Stdin => {
                cmd.stdin(Stdio::piped());
            }
            PromptVia::Flag(flag) => {
                cmd.arg(flag).arg(prompt).stdin(Stdio::null());
            }
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning observer consult ({})", self.program))?;
        if self.prompt_via == PromptVia::Stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes()).await?;
                drop(stdin);
            }
        }
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("observer consult timed out after {}s", timeout.as_secs()))??;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "observer consult exited {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim().chars().take(400).collect::<String>()
            );
        }
        if stdout.is_empty() {
            bail!("observer consult produced no output");
        }
        Ok(stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_gets_plan_mode_and_read_tools_only() {
        let c = resolve_read_only("claude --model opus", false).unwrap();
        assert_eq!(c.profile, "claude");
        let joined = c.args.join(" ");
        assert!(joined.contains("--permission-mode plan"));
        assert!(joined.contains(&format!("--tools {CLAUDE_READ_TOOLS}")));
        assert!(joined.contains(&format!("--disallowedTools {CLAUDE_DENIED_TOOLS}")));
        assert!(joined.contains("--strict-mcp-config"));
        assert!(joined.ends_with("--model opus"));
    }

    #[test]
    fn known_clis_refuse_unknown_flags() {
        for cmd in [
            "claude --dangerously-skip-permissions",
            "claude --permission-mode bypassPermissions",
            "kimi --yolo",
            "codex --sandbox danger-full-access",
        ] {
            assert!(
                resolve_read_only(cmd, true).is_err(),
                "{cmd} must be refused"
            );
        }
    }

    #[test]
    fn kimi_and_codex_profiles() {
        let k = resolve_read_only("kimi", false).unwrap();
        assert_eq!(k.args, vec!["--plan"]);
        assert_eq!(k.prompt_via, PromptVia::Flag("-p"));
        let c = resolve_read_only("/usr/local/bin/codex -m o3", false).unwrap();
        assert_eq!(c.args[..3], ["exec", "--sandbox", "read-only"]);
        assert_eq!(c.args.last().unwrap(), "-");
    }

    #[test]
    fn unknown_command_needs_attestation() {
        assert!(resolve_read_only("some-advisor", false).is_err());
        let a = resolve_read_only("my-advisor --x", true).unwrap();
        assert_eq!(a.profile, "attested");
        assert_eq!(a.args, vec!["-c", "my-advisor --x"]);
    }

    #[tokio::test]
    async fn attested_run_reads_stdin_and_fails_on_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = resolve_read_only("cat", true).unwrap();
        let out = ok
            .run(tmp.path(), "hello", Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(out, "hello");
        let bad = resolve_read_only("exit 3", true).unwrap();
        assert!(bad
            .run(tmp.path(), "x", Duration::from_secs(10))
            .await
            .is_err());
    }
}
