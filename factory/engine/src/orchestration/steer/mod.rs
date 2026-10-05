//! Steering checkpoint / consult / commit-gate support for Rails.
//!
//! This is the Rust-side replacement for the `.steering/bin/steer-stop.sh` and
//! `steer-pre-commit-gate.sh` shell hooks.  It keeps the same semantics:
//! - `checkpoint` hashes `.steering/checkpoint.md` and emits a ledger event when
//!   it changes.
//! - `consult` builds a prompt context and invokes an external steering agent.
//! - `commit-gate` runs a consult specialized for a pending git commit/push.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;

use crate::core::ids::create_event_id;
use crate::core::types::{Actor, ActorType, AllternitEvent};
use crate::ledger::Ledger;
use std::sync::Arc;

/// Hash a checkpoint file.  Uses a fast stable hash; the value is advisory.
pub fn hash_checkpoint(contents: &str) -> String {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Result of a checkpoint call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointResult {
    pub changed: bool,
    pub hash: String,
    pub event_id: Option<String>,
}

/// Steering coordinator.
pub struct Steer {
    ledger: Arc<Ledger>,
    actor: Actor,
}

impl Steer {
    pub fn new(ledger: Arc<Ledger>) -> Self {
        Self {
            ledger,
            actor: Actor {
                r#type: ActorType::Gate,
                id: "steer".to_string(),
            },
        }
    }

    /// Read `.steering/checkpoint.md` under `cwd`, hash it, and emit a
    /// `SteeringCheckpoint` ledger event when the hash differs from the last
    /// recorded hash (stored in `.steering/state/checkpoint.hash`).
    pub async fn checkpoint(&self, cwd: impl AsRef<Path>) -> Result<CheckpointResult> {
        let cwd = cwd.as_ref();
        let checkpoint_file = cwd.join(".steering").join("checkpoint.md");
        let state_dir = cwd.join(".steering").join("state");
        let hash_file = state_dir.join("checkpoint.hash");

        if !checkpoint_file.exists() {
            anyhow::bail!("checkpoint file not found: {}", checkpoint_file.display());
        }

        fs::create_dir_all(&state_dir)
            .with_context(|| format!("creating state dir {}", state_dir.display()))?;

        let contents = fs::read_to_string(&checkpoint_file)
            .with_context(|| format!("reading {}", checkpoint_file.display()))?;
        let hash = hash_checkpoint(&contents);

        let last_hash = fs::read_to_string(&hash_file).unwrap_or_default().trim().to_string();
        let changed = last_hash.is_empty() || last_hash != hash;

        let event_id = if changed {
            let event = AllternitEvent {
                event_id: create_event_id(),
                ts: Utc::now().to_rfc3339(),
                actor: self.actor.clone(),
                scope: None,
                r#type: "SteeringCheckpoint".to_string(),
                payload: json!({
                    "cwd": cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()).display().to_string(),
                    "hash": hash,
                    "previous_hash": last_hash,
                    "size_bytes": contents.len(),
                }),
                provenance: None,
            };
            let id = self.ledger.append(event).await?;
            fs::write(&hash_file, &hash)
                .with_context(|| format!("writing {}", hash_file.display()))?;
            Some(id)
        } else {
            None
        };

        Ok(CheckpointResult {
            changed,
            hash,
            event_id,
        })
    }

    /// Build the steering consult prompt context from:
    /// - `.steering/prompt.md`
    /// - `.steering/spec.md` (source of truth)
    /// - `.steering/checkpoint.md`
    /// - git status / diff evidence
    /// - optional `.steering/test-command` output
    pub fn build_context(&self, cwd: impl AsRef<Path>) -> Result<String> {
        let cwd = cwd.as_ref();
        let steering_dir = cwd.join(".steering");
        let prompt_path = steering_dir.join("prompt.md");
        let spec_path = steering_dir.join("spec.md");
        let checkpoint_path = steering_dir.join("checkpoint.md");
        let test_command = steering_dir.join("test-command");

        let mut context = String::new();

        if prompt_path.exists() {
            context.push_str(&fs::read_to_string(&prompt_path)?);
        }

        if spec_path.exists() {
            context.push_str("\n\n=== SPEC FILE (.steering/spec.md) ===\n");
            let spec = fs::read_to_string(&spec_path)?;
            context.push_str(&truncate(&spec, 12_000));
        }

        if checkpoint_path.exists() {
            context.push_str("\n\n=== CHECKPOINT FILE (.steering/checkpoint.md) ===\n");
            let checkpoint = fs::read_to_string(&checkpoint_path)?;
            context.push_str(&truncate(&checkpoint, 12_000));
        }

        context.push_str("\n\n=== EVIDENCE: git status --short ===\n");
        if let Ok(output) = std::process::Command::new("git")
            .args(["-C", &cwd.to_string_lossy(), "status", "--short"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().take(50) {
                context.push_str(line);
                context.push('\n');
            }
        }

        context.push_str("\n=== EVIDENCE: git diff --stat HEAD ===\n");
        if let Ok(output) = std::process::Command::new("git")
            .args(["-C", &cwd.to_string_lossy(), "diff", "--stat", "HEAD"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            context.push_str(&truncate(&text, 2_000));
        }

        context.push_str("\n\n=== EVIDENCE: git diff HEAD (first 16KB) ===\n");
        if let Ok(output) = std::process::Command::new("git")
            .args(["-C", &cwd.to_string_lossy(), "diff", "HEAD"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            context.push_str(&truncate(&text, 16_384));
        }

        if test_command.exists() {
            context.push_str("\n\n=== EVIDENCE: test output (`.steering/test-command`, tail) ===\n");
            if let Ok(output) = std::process::Command::new("bash")
                .arg(&test_command)
                .current_dir(cwd)
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                context.push_str(&truncate(&text, 4_096));
                context.push_str(&format!("\ntest-command exit: {}\n", output.status.code().unwrap_or(-1)));
            }
        }

        Ok(context)
    }

    /// Invoke the configured steering consult command with the provided context.
    /// Returns the raw answer text (first line can be checked for APPROVE/STEER).
    pub async fn consult(&self, cwd: impl AsRef<Path>, context: &str) -> Result<String> {
        let cwd = cwd.as_ref();

        if let Ok(cmd) = std::env::var("STEER_CONSULT_CMD") {
            return run_shell_command(&cmd, cwd, context).await;
        }

        // Recursion guard: `ao-consult` is a shim that execs `allternit-rails
        // steer consult`, which would call back into the shim without bound
        // (2026-10-02 fork bomb). When AO_CONSULT_ACTIVE is set (we are already
        // inside a consult, or the shim set it), skip the shim and fall
        // through to the kimi fallback.
        let consult_active = std::env::var("AO_CONSULT_ACTIVE")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !consult_active && command_exists("ao-consult") {
            return run_shell_command("ao-consult", cwd, context).await;
        }

        // Fallback: kimi -p <prompt>
        if command_exists("kimi") {
            let child = Command::new("kimi")
                .arg("-p")
                .arg(context)
                .current_dir(cwd)
                .env("AO_CONSULT_ACTIVE", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("spawning kimi consult")?;
            let output = child.wait_with_output().await.context("waiting for kimi")?;
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        anyhow::bail!("no steering consult backend found (set STEER_CONSULT_CMD, or install ao-consult/kimi)")
    }

    /// Run a commit-gate consult.  Returns the first-line verdict and full body.
    pub async fn commit_gate(&self, cwd: impl AsRef<Path>) -> Result<ConsultResult> {
        let cwd = cwd.as_ref();
        let mut context = self.build_context(cwd)?;
        context.push_str("\n\nThis is a COMMIT GATE consult. Approve only if the diff is safe, scoped, and matches the spec. Reply with APPROVE or STEER followed by reasoning and any required fixes.");
        let answer = self.consult(cwd, &context).await?;
        Ok(parse_verdict(&answer))
    }
}

/// Result of a consult / commit-gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsultResult {
    pub verdict: String,
    pub body: String,
}

fn truncate(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        s.to_string()
    } else {
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n[truncated]", &s[..end])
    }
}

fn command_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn run_shell_command(cmd: &str, cwd: &Path, stdin: &str) -> Result<String> {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        // Recursion guard: nested steering consults triggered by hooks inside
        // this child must not re-enter the ao-consult shim.
        .env("AO_CONSULT_ACTIVE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning consult command: {}", cmd))?;

    if let Some(mut child_stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        child_stdin.write_all(stdin.as_bytes()).await?;
    }

    let output = child.wait_with_output().await?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_verdict(answer: &str) -> ConsultResult {
    let lines: Vec<&str> = answer.lines().collect();
    let first = lines
        .first()
        .map(|s| s.trim().trim_start_matches("• ").to_uppercase())
        .unwrap_or_default();
    let verdict = if first.starts_with("APPROVE") {
        "APPROVE".to_string()
    } else {
        "STEER".to_string()
    };
    ConsultResult {
        verdict,
        body: answer.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::ledger::LedgerOptions;
    use std::ffi::OsString;
    use std::io::Write;

    /// Serializes env mutation (PATH / STEER_CONSULT_CMD / AO_CONSULT_ACTIVE)
    /// across the consult tests in this module.
    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Saves the consult-related env vars and restores them on drop (including
    /// on panic), so a failing test cannot leak mutated env into other tests.
    struct EnvGuard {
        path: Option<OsString>,
        cmd: Option<OsString>,
        active: Option<OsString>,
    }

    impl EnvGuard {
        fn takeover() -> Self {
            let guard = Self {
                path: std::env::var_os("PATH"),
                cmd: std::env::var_os("STEER_CONSULT_CMD"),
                active: std::env::var_os("AO_CONSULT_ACTIVE"),
            };
            std::env::remove_var("STEER_CONSULT_CMD");
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match &self.cmd {
                Some(v) => std::env::set_var("STEER_CONSULT_CMD", v),
                None => std::env::remove_var("STEER_CONSULT_CMD"),
            }
            match &self.active {
                Some(v) => std::env::set_var("AO_CONSULT_ACTIVE", v),
                None => std::env::remove_var("AO_CONSULT_ACTIVE"),
            }
        }
    }

    /// Prepends `bin_dir` to PATH.
    fn prepend_path(bin_dir: &Path) {
        let mut v = bin_dir.as_os_str().to_os_string();
        if let Some(old) = std::env::var_os("PATH") {
            v.push(":");
            v.push(old);
        }
        std::env::set_var("PATH", v);
    }

    fn write_script(path: &Path, body: &str) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn test_steer(root: &Path) -> Steer {
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(root.to_path_buf()),
            ledger_dir: None,
        }));
        Steer::new(ledger)
    }

    /// With AO_CONSULT_ACTIVE set, consult() must not invoke an ao-consult on
    /// PATH (even though one is present) and must fall through to the kimi
    /// fallback. Regression test for the 2026-10-02 steer-consult fork bomb:
    /// ao-consult execs `allternit-rails steer consult`, which called
    /// ao-consult again without bound.
    #[tokio::test]
    async fn consult_skips_ao_consult_when_active() {
        let _guard = env_lock().lock().await;

        let root = tempfile::tempdir().unwrap();
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        // Fake ao-consult that would fail loudly if invoked.
        let shim = bin_dir.join("ao-consult");
        write_script(
            &shim,
            "#!/usr/bin/env bash\necho AO-CONSULT-INVOKED >&2\nexit 42\n",
        );
        // Fake kimi that answers successfully; the expected fallback backend.
        let kimi = bin_dir.join("kimi");
        write_script(&kimi, "#!/usr/bin/env bash\necho KIMI-ANSWER\n");

        let _env = EnvGuard::takeover();
        prepend_path(&bin_dir);
        std::env::set_var("AO_CONSULT_ACTIVE", "1");

        let answer = test_steer(root.path())
            .consult(root.path(), "test context")
            .await
            .expect("consult failed");

        assert!(
            answer.contains("KIMI-ANSWER"),
            "expected kimi fallback answer, got: {answer:?}"
        );
        assert!(
            !answer.contains("AO-CONSULT-INVOKED"),
            "ao-consult shim was invoked despite AO_CONSULT_ACTIVE"
        );
    }

    /// Without AO_CONSULT_ACTIVE, an ao-consult on PATH is used (guards
    /// against accidentally disabling the primary backend).
    #[tokio::test]
    async fn consult_uses_ao_consult_when_not_active() {
        let _guard = env_lock().lock().await;

        let root = tempfile::tempdir().unwrap();
        let bin_dir = root.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let shim = bin_dir.join("ao-consult");
        write_script(
            &shim,
            "#!/usr/bin/env bash\nstdin=$(cat)\necho AO-CONSULT-USED\n",
        );

        let _env = EnvGuard::takeover();
        prepend_path(&bin_dir);
        std::env::remove_var("AO_CONSULT_ACTIVE");

        let answer = test_steer(root.path())
            .consult(root.path(), "test context")
            .await
            .expect("consult failed");

        assert!(
            answer.contains("AO-CONSULT-USED"),
            "expected ao-consult answer, got: {answer:?}"
        );
    }
}
