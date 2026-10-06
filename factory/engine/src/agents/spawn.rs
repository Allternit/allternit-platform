//! The one spawn path: a gated agent session in a pane-engine pane.
//!
//! This replaced the old orchestrator's tmux spawner. What a spawn does
//! is unchanged (spawn gate, WIH policy, execution environment, fence, hook
//! settings, peer registration, headless capture); where it runs is now always
//! the pane engine (`factory/pane`) through the installed [`PaneBackend`], and
//! every spawn is recorded in the session [`Registry`].
//!
//! The hook a gated harness runs is this engine's own executable
//! (`allternit-factory internal hook …`), resolved by
//! [`crate::hook::find_factory_bin`]; a spawn whose hook binary can't be
//! resolved is refused, never run unhooked.
//!
//! [`PaneBackend`]: super::backend::PaneBackend

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::Utc;
use tokio::process::Command;
use tokio::time::sleep;

use super::backend::{self, PaneSpawn};
use super::registry::{self, BotRef, Entry, Registry};
use crate::core::io::ensure_dir;
use crate::hook::{self, HarnessGate, HookTarget};
use crate::ledger::{Ledger, LedgerOptions};
use crate::peer::PeerRegistry;

/// Options for spawning an agent session.
pub struct SpawnOptions<'a> {
    pub slug: &'a str,
    pub repo: &'a Path,
    pub cmd: &'a [String],
    pub worktree: bool,
    /// The harness (`claude`, `codex`, …), recorded on the peer and the registry.
    pub vendor: &'a str,
    #[allow(dead_code)]
    pub mode: &'a str,
    pub task_file: Option<&'a Path>,
    pub notes_sentinel: Option<&'a Path>,
    /// WIH the spawned session is bound to. Hooked harnesses enforce Gate 2
    /// against it; unhooked ones are refused when its policy needs leased writes.
    pub wih: Option<&'a str>,
    /// Headless capture (used by `drive`): the harness's stdout/stderr go to
    /// these files, stdin is `/dev/null`, and its exit code is written to
    /// `exit_code` (atomically, via a `.tmp` rename) when it finishes. The
    /// exit-code file doubles as the completion sentinel for [`Spawner::poll`].
    pub capture: Option<&'a CaptureFiles>,
    /// The bot this session belongs to; `None` gets a local placeholder.
    pub bot: Option<BotRef>,
}

/// Files a captured (headless) spawn writes. See [`SpawnOptions::capture`].
#[derive(Debug, Clone)]
pub struct CaptureFiles {
    pub stdout: PathBuf,
    pub stderr: PathBuf,
    pub exit_code: PathBuf,
}

/// Result of a successful spawn.
pub struct SpawnResult {
    pub session: String,
    pub pane_id: String,
    pub workdir: PathBuf,
    pub logfile: PathBuf,
    pub peer_id: String,
    pub inbox_socket: PathBuf,
}

/// Watch outcome.
pub enum WatchOutcome {
    Done,
    Dead,
    Timeout,
}

/// Starts and stops agent sessions for one workspace root.
pub struct Spawner {
    root_dir: PathBuf,
    peers: Arc<PeerRegistry>,
    registry: Registry,
}

impl Spawner {
    pub fn new(root_dir: PathBuf) -> Result<Self> {
        let peers = Arc::new(PeerRegistry::new(&root_dir)?);
        Ok(Self { root_dir, peers, registry: Registry::open_default() })
    }

    /// Use another registry file (tests).
    pub fn with_registry(mut self, registry: Registry) -> Self {
        self.registry = registry;
        self
    }

    /// Spawn a gated agent session in a pane and register it.
    pub async fn spawn(&self, opts: SpawnOptions<'_>) -> Result<SpawnResult> {
        let slug = sanitize_slug(opts.slug);
        let session = registry::session_of(&slug);
        let logdir = registry::logs_dir();
        ensure_dir(&logdir)?;
        let log = logdir.join(format!("{}-{}.log", session, Utc::now().format("%Y%m%d-%H%M%S")));
        let runner = logdir.join(format!("{}.cmd.sh", session));

        let pane = backend::backend()?;
        {
            let pane = pane.clone();
            let probe = session.clone();
            if backend::blocking(move || pane.find(&probe)).await?.is_some() {
                bail!("session {} already exists (agents ps to inspect)", session);
            }
        }

        // Spawn gate (audit S1): admission before any side effect.
        let harness = opts.cmd.first().map(String::as_str).unwrap_or("");
        let ledger = Ledger::new(LedgerOptions {
            root_dir: Some(self.root_dir.clone()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        });
        let wih_policy = match opts.wih {
            Some(wih_id) => Some(hook::load_wih_policy(&ledger, wih_id).await?),
            None => None,
        };
        let gate = match hook::admit(harness, wih_policy.as_ref()) {
            Ok(gate) => gate,
            Err(reason) => {
                if let Some(wih_id) = opts.wih {
                    let _ = ledger.append(hook::spawn_refused_event(harness, wih_id, &reason)).await;
                }
                bail!(reason);
            }
        };
        // A hooked harness without the engine binary would run unhooked;
        // refuse instead of falling back to bypass.
        let gate_bin = if gate == HarnessGate::Hook {
            Some(hook::find_factory_bin().ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot install the spawn-gate hook for {harness}: this process is not allternit-factory and ALLTERNIT_FACTORY_BIN is not set"
                )
            })?)
        } else {
            None
        };

        let (workdir, wt_created) = if opts.worktree {
            create_worktree(opts.repo, &slug).await?
        } else {
            (opts.repo.to_path_buf(), false)
        };
        let cleanup = |workdir: PathBuf| async move {
            if wt_created {
                let _ = remove_worktree(&workdir).await;
            }
        };

        let hook_target = gate_bin.as_deref().map(|bin| HookTarget {
            factory_bin: bin,
            root: &self.root_dir,
            workspace: Some(&workdir),
            wih_id: opts.wih,
        });
        let settings_path = match hook_target.and_then(|t| hook::hook_settings_file(harness, t)) {
            Some((suffix, settings)) => {
                let path = logdir.join(format!("{}.{}", session, suffix));
                if let Err(err) = tokio::fs::write(&path, serde_json::to_string_pretty(&settings)?).await {
                    cleanup(workdir.clone()).await;
                    bail!("writing spawn-gate settings {}: {}", path.display(), err);
                }
                Some(path)
            }
            None => None,
        };
        let gated = hook::gate_spawn(opts.cmd, settings_path.as_deref(), hook_target);
        let cmd = gated.argv;

        // ExecutionEnvironmentV1: resolve per node, write beside the log, and
        // record in the ledger. `ALLTERNIT_EXEC_ENV_ENFORCE=1` additionally
        // strips the node's process env to the allowlist (`env -i`).
        let node_env = crate::execenv::resolve(&crate::execenv::EnvRequest {
            node_id: &session,
            workdir: &workdir,
            worktree: opts.worktree,
            extra_env_keys: &std::env::var("ALLTERNIT_EXEC_ENV_ALLOW")
                .map(|v| v.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect::<Vec<_>>())
                .unwrap_or_default(),
            mounts: &[],
            secret_refs: &[],
            network_policy_id: None,
        });
        let _ = tokio::fs::write(
            logdir.join(format!("{}.exec-env.json", session)),
            serde_json::to_string_pretty(&node_env).unwrap_or_default(),
        )
        .await;
        let _ = ledger.append(crate::execenv::resolved_event(opts.wih, &node_env)).await;

        // Register the peer first so the pane's env names a known inbox.
        let peer = self.peers.register(&session, workdir.clone(), opts.vendor)?;

        // The pane's own environment (the old tmux `export …` prefix).
        let fence_strict = hook::fence_env_strict() || wih_policy.as_ref().is_some_and(|p| p.fence_strict);
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        let inbox = peer.inbox_socket.to_string_lossy().to_string();
        let root_s = workdir.to_string_lossy().to_string();
        // The pane slug, the same value a team bot's pane carries (whoami.rs).
        env.insert("ALLTERNIT_FACTORY_PANE_ID".into(), super::registry::slug_of(&session).to_string());
        for (k, v) in [
            ("ALLTERNIT_FACTORY_PEER_NAME", &peer.name),
            ("ALLTERNIT_FACTORY_INBOX", &inbox),
            ("ALLTERNIT_FACTORY_ROOT", &root_s),
        ] {
            env.insert(k.into(), v.clone());
        }
        if let Some(wih_id) = opts.wih {
            env.insert("ALLTERNIT_FACTORY_WIH".into(), wih_id.to_string());
        }
        if fence_strict {
            env.insert(hook::FENCE_ENV.into(), "strict".into());
        }
        if let Some(task) = opts.task_file {
            let task = task.to_string_lossy().to_string();
            env.insert("ALLTERNIT_FACTORY_TASK_FILE".into(), task);
        }
        // Claude refuses bypassPermissions as root unless told it is in a
        // sandbox; Allternit's execution environment is that sandbox.
        let mut gate_env = gated.env;
        if hook::HookFlavor::of(harness) == Some(hook::HookFlavor::Claude) {
            gate_env.push(("IS_SANDBOX".to_string(), "1".to_string()));
        }

        // The runner file sidesteps quoting; the env allowlist (when enforced)
        // wraps it so only allowlisted and per-session names survive.
        let env_prefix = if fence_strict || std::env::var("ALLTERNIT_EXEC_ENV_ENFORCE").as_deref() == Ok("1") {
            let mut p = String::from("env -i");
            for (k, v) in crate::execenv::filter_env(&node_env, std::env::vars()) {
                p.push_str(&format!(" {}={}", k, shell_escape(&v)));
            }
            for k in env.keys() {
                p.push_str(&format!(" {k}=\"${k}\""));
            }
            p.push(' ');
            p
        } else {
            String::new()
        };
        let env_prefix = if gate_env.is_empty() {
            env_prefix
        } else {
            let mut p = if env_prefix.is_empty() { "env ".to_string() } else { env_prefix };
            for (k, v) in &gate_env {
                p.push_str(&format!("{k}={} ", shell_escape(v)));
            }
            p
        };
        let mut runner_text = format!(
            "{}{}",
            env_prefix,
            cmd.iter().map(|s| shell_escape(s)).collect::<Vec<_>>().join(" ")
        );
        if let Some(cap) = opts.capture {
            let tmp = cap.exit_code.with_extension("tmp");
            runner_text = format!(
                "{} < /dev/null > {} 2> {}\nprintf '%s\\n' \"$?\" > {} && mv {} {}",
                runner_text,
                shell_escape(&cap.stdout.to_string_lossy()),
                shell_escape(&cap.stderr.to_string_lossy()),
                shell_escape(&tmp.to_string_lossy()),
                shell_escape(&tmp.to_string_lossy()),
                shell_escape(&cap.exit_code.to_string_lossy()),
            );
        }
        if let Some(sentinel) = opts.notes_sentinel {
            runner_text.push_str(&format!("\ntouch {}", shell_escape(&sentinel.to_string_lossy())));
        }
        tokio::fs::write(&runner, format!("{}\n", runner_text)).await?;

        let request = PaneSpawn {
            session: session.clone(),
            cwd: workdir.clone(),
            argv: vec!["/bin/sh".to_string(), runner.to_string_lossy().to_string()],
            env,
            transcript: Some(log.clone()),
        };
        let spawned = {
            let pane = pane.clone();
            backend::blocking(move || pane.spawn(&request)).await
        };
        let live = match spawned {
            Ok(live) => live,
            Err(err) => {
                self.peers.unregister(&peer.peer_id).ok();
                cleanup(workdir.clone()).await;
                return Err(err.context(format!("starting the pane for {session}")));
            }
        };

        let now = Utc::now().to_rfc3339();
        let mut entry = Entry {
            cwd: workdir.to_string_lossy().to_string(),
            log: Some(log.to_string_lossy().to_string()),
            runner: Some(runner.to_string_lossy().to_string()),
            worktree: opts.worktree.then(|| workdir.to_string_lossy().to_string()),
            branch: opts.worktree.then(|| format!("ao/{slug}")),
            sentinel: opts.notes_sentinel.map(|p| p.to_string_lossy().to_string()),
            lead: Some(std::env::var("ALLTERNIT_FACTORY_LEAD").unwrap_or_else(|_| "engine".to_string())),
            lifecycle: Some("running".to_string()),
            world: Some("engine".to_string()),
            pane_id: Some(live.pane_id.clone()),
            harness: Some(opts.vendor.to_string()).filter(|h| !h.is_empty()),
            bot: Some(opts.bot.clone().unwrap_or_else(|| BotRef::placeholder(&slug))),
            created_at: Some(now.clone()),
            updated_at: Some(now),
            ..Default::default()
        };

        sleep(Duration::from_millis(500)).await;
        // A captured run that already wrote its exit code finished fast (the
        // pane engine can have removed the pane already); that is a completed
        // run, not a failed spawn.
        let finished_fast = opts.capture.is_some_and(|c| c.exit_code.exists());
        let still_live = {
            let pane = pane.clone();
            let probe = session.clone();
            backend::blocking(move || pane.find(&probe)).await?.is_some()
        };
        if !finished_fast && !still_live {
            entry.dead = true;
            entry.lifecycle = Some("dead".to_string());
            let _ = self.registry.upsert(&session, entry);
            self.peers.unregister(&peer.peer_id).ok();
            cleanup(workdir.clone()).await;
            let tail = tokio::fs::read_to_string(&log).await.unwrap_or_default();
            let mut lines: Vec<&str> = tail.lines().rev().take(5).collect();
            lines.reverse();
            bail!("agent exited immediately — transcript tail:\n{}", lines.join("\n"));
        }
        if finished_fast && !still_live {
            entry.dead = true;
            entry.lifecycle = Some("finished".to_string());
        }
        self.registry.upsert(&session, entry)?;

        Ok(SpawnResult {
            session,
            pane_id: live.pane_id,
            workdir,
            logfile: log,
            peer_id: peer.peer_id,
            inbox_socket: peer.inbox_socket,
        })
    }

    /// One non-blocking watch step: `Some(Done)` when the sentinel exists,
    /// `Some(Dead)` when the pane is gone, `None` while it is still running
    /// (or the pane engine can't be asked right now). Timeouts are the
    /// caller's clock.
    pub async fn poll(&self, slug: &str, sentinel: &Path) -> Option<WatchOutcome> {
        if sentinel.exists() {
            return Some(WatchOutcome::Done);
        }
        match session_live(slug).await {
            Ok(true) | Err(_) => None,
            // The run can write its sentinel and exit between the two checks.
            Ok(false) if sentinel.exists() => Some(WatchOutcome::Done),
            Ok(false) => Some(WatchOutcome::Dead),
        }
    }

    /// True while the session's pane is live.
    pub async fn is_alive(&self, slug: &str) -> bool {
        session_alive(slug).await
    }

    /// Close a session's pane, unregister its peer and mark its record.
    pub async fn kill(&self, slug: &str, rm_worktree: bool) -> Result<()> {
        let session = session_name(slug);
        let pane = backend::backend()?;
        let found = {
            let pane = pane.clone();
            let probe = session.clone();
            backend::blocking(move || pane.find(&probe)).await?
        };
        let wt_dir = found.as_ref().and_then(|p| p.cwd.clone());
        if found.is_some() {
            let probe = session.clone();
            backend::blocking(move || pane.kill(&probe)).await?;
        }
        self.peers.unregister(&session).ok();
        let now = Utc::now().to_rfc3339();
        self.registry.update(|file| {
            if let Some(entry) = file.sessions.get_mut(&session) {
                entry.dead = true;
                if entry.lifecycle.as_deref() != Some("finished") {
                    entry.lifecycle = Some("dead".to_string());
                }
                entry.updated_at = Some(now);
            }
        })?;
        if rm_worktree {
            if let Some(dir) = wt_dir {
                let dir_path = Path::new(&dir);
                if is_ao_worktree(dir_path, slug) {
                    remove_worktree(dir_path).await?;
                }
            }
        }
        Ok(())
    }
}

/// Whether the session's pane is live, or an error when the pane engine
/// can't be asked.
pub async fn session_live(slug: &str) -> Result<bool> {
    let pane = backend::backend()?;
    let session = session_name(slug);
    Ok(backend::blocking(move || pane.find(&session)).await?.is_some())
}

/// True when the session's pane is live (false when it can't be checked).
pub async fn session_alive(slug: &str) -> bool {
    session_live(slug).await.unwrap_or(false)
}

/// Blocking form of [`session_alive`] for code holding a file lock.
pub fn session_alive_blocking(slug: &str) -> bool {
    let session = session_name(slug);
    let Ok(pane) = backend::backend() else { return false };
    std::thread::spawn(move || pane.find(&session).map(|p| p.is_some()).unwrap_or(false))
        .join()
        .unwrap_or(false)
}

/// Session-name form of a slug (`ao-<sanitized>`).
pub fn session_name(slug: &str) -> String {
    registry::session_of(&sanitize_slug(slug))
}

pub fn sanitize_slug(slug: &str) -> String {
    slug.trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
}

fn shell_escape(s: &str) -> String {
    // POSIX single quotes: everything inside is literal, including
    // backslashes. Only `'` needs closing, escaping and reopening.
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

async fn create_worktree(repo: &Path, slug: &str) -> Result<(PathBuf, bool)> {
    let root = git_toplevel(repo).await?;
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    if root == home {
        bail!("git root is $HOME — a worktree would checkout your whole home dir");
    }
    let wt = root
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join(format!("{}-ao-{}", root.file_name().unwrap_or_default().to_string_lossy(), slug));
    let status = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["worktree", "add"])
        .arg(&wt)
        .arg("-b")
        .arg(format!("ao/{}", slug))
        .status()
        .await?;
    if !status.success() {
        bail!("git worktree add failed");
    }
    Ok((wt, true))
}

async fn remove_worktree(wt: &Path) -> Result<()> {
    let status = Command::new("git").args(["worktree", "remove", "--force"]).arg(wt).status().await;
    if status.map(|s| s.success()).unwrap_or(false) {
        return Ok(());
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(wt)
        .args(["worktree", "remove", "--force"])
        .arg(wt)
        .status()
        .await?;
    if !status.success() {
        bail!("could not remove worktree {}", wt.display());
    }
    Ok(())
}

fn is_ao_worktree(dir: &Path, slug: &str) -> bool {
    dir.to_string_lossy().ends_with(&format!("-ao-{}", slug))
}

async fn git_toplevel(repo: &Path) -> Result<PathBuf> {
    let output = Command::new("git").arg("-C").arg(repo).args(["rev-parse", "--show-toplevel"]).output().await?;
    if !output.status.success() {
        bail!("not a git repository: {}", repo.display());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_sanitization() {
        assert_eq!(sanitize_slug("My Session"), "my-session");
        assert_eq!(sanitize_slug("foo_bar-1"), "foo_bar-1");
        assert_eq!(sanitize_slug("--trim--"), "trim");
    }

    #[test]
    fn shell_escape_round_trips_through_sh() {
        for word in ["plain", "it's", r"back\slash", r"sh -c 'a'\''b'", "$(nope) `x` \"q\""] {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {}", shell_escape(word)))
                .output()
                .expect("sh");
            assert_eq!(String::from_utf8_lossy(&out.stdout), word);
        }
    }
}
