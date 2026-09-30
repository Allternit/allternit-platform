//! Native agent orchestrator backed by tmux and the Rails peer registry.
//!
//! This module implements the `allternit-rails orchestrator` subcommands.  It
//! preserves the semantics of the legacy `ao-*` bash scripts while recording
//! every session in the Rails peer registry so other local agents can discover
//! and message it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::Utc;
use tokio::process::Command;
use tokio::time::{sleep, Instant};

use crate::core::io::ensure_dir;
use crate::hook::{self, HarnessGate, HookTarget};
use crate::ledger::{Ledger, LedgerOptions};
use crate::peer::{PeerEnvelope, PeerRegistry, PeerStatus, send_envelope};

/// Options for spawning an executor session.
pub struct SpawnOptions<'a> {
    pub slug: &'a str,
    pub repo: &'a Path,
    pub cmd: &'a [String],
    pub worktree: bool,
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
    /// exit-code file doubles as the completion sentinel for [`Orchestrator::poll`].
    pub capture: Option<&'a CaptureFiles>,
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

/// Per-executor health report.
pub struct DoctorReport {
    pub transport_ok: bool,
    pub executors: Vec<ExecutorProbe>,
}

#[derive(Debug, Clone)]
pub struct ExecutorProbe {
    pub vendor: String,
    pub binary: String,
    pub installed: bool,
    pub version: Option<String>,
    pub interactive_flags_ok: bool,
    pub headless_flags_ok: bool,
    pub note: String,
}

/// Orchestrator for local CLI agent sessions.
pub struct Orchestrator {
    root_dir: PathBuf,
    registry: Arc<PeerRegistry>,
}

impl Orchestrator {
    /// Open the orchestrator for `root_dir`.
    pub fn new(root_dir: PathBuf) -> Result<Self> {
        let registry = Arc::new(PeerRegistry::new(&root_dir)?);
        Ok(Self { root_dir, registry })
    }

    /// Spawn a new executor tmux session and register it as a peer.
    pub async fn spawn(&self, opts: SpawnOptions<'_>) -> Result<SpawnResult> {
        let slug = sanitize_slug(opts.slug);
        let session = format!("ao-{}", slug);
        let logdir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".agent-orchestrator")
            .join("logs");
        ensure_dir(&logdir)?;
        let log = logdir.join(format!(
            "{}-{}.log",
            session,
            Utc::now().format("%Y%m%d-%H%M%S")
        ));
        let runner = logdir.join(format!("{}.cmd.sh", session));

        if tmux_has_session(&session).await? {
            bail!(
                "session {} already exists (orchestrator status {} to inspect)",
                session,
                slug
            );
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
                    let _ = ledger
                        .append(hook::spawn_refused_event(harness, wih_id, &reason))
                        .await;
                }
                bail!(reason);
            }
        };
        // A hooked harness without a reachable hook binary would run unhooked;
        // refuse instead of falling back to bypass.
        let commrails_bin = if gate == HarnessGate::Hook {
            Some(hook::find_commrails_bin().ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot install the spawn-gate hook for {harness}: allternit-commrails binary not found (set ALLTERNIT_COMMRAILS_BIN)"
                )
            })?)
        } else {
            None
        };

        let (workdir, wt_created) = if opts.worktree {
            let (wt, created) = create_worktree(opts.repo, &slug).await?;
            (wt, created)
        } else {
            (opts.repo.to_path_buf(), false)
        };

        let settings_path = match &commrails_bin {
            Some(bin) => {
                let path = logdir.join(format!("{}.claude-settings.json", session));
                let settings = hook::claude_settings(HookTarget {
                    commrails_bin: bin,
                    root: &self.root_dir,
                    workspace: Some(&workdir),
                    wih_id: opts.wih,
                });
                if let Err(err) = tokio::fs::write(&path, serde_json::to_string_pretty(&settings)?).await {
                    if wt_created {
                        let _ = remove_worktree(&workdir).await;
                    }
                    bail!("writing spawn-gate settings {}: {}", path.display(), err);
                }
                Some(path)
            }
            None => None,
        };
        let cmd = hook::gate_argv(opts.cmd, settings_path.as_deref());

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
        let env_prefix = if std::env::var("ALLTERNIT_EXEC_ENV_ENFORCE").as_deref() == Ok("1") {
            let kept = crate::execenv::filter_env(&node_env, std::env::vars());
            let mut p = String::from("env -i");
            for (k, v) in kept {
                p.push_str(&format!(" {}={}", k, shell_escape(&v)));
            }
            // The per-session exports above live in the tmux shell; carry them across.
            let mut names = vec![
                "ALLTERNIT_AO_PANE_ID", "ALLTERNIT_COMMRAILS_PEER_NAME", "ALLTERNIT_COMMRAILS_INBOX",
                "ALLTERNIT_COMMRAILS_ROOT", "ALLTERNIT_RAILS_PEER_NAME", "ALLTERNIT_RAILS_INBOX", "ALLTERNIT_RAILS_ROOT",
            ];
            if opts.wih.is_some() {
                names.push("ALLTERNIT_COMMRAILS_WIH");
            }
            if opts.task_file.is_some() {
                names.extend(["ALLTERNIT_COMMRAILS_TASK_FILE", "ALLTERNIT_RAILS_TASK_FILE"]);
            }
            for n in names {
                p.push_str(&format!(" {n}=\"${n}\""));
            }
            p.push(' ');
            p
        } else {
            String::new()
        };

        // Write the runner file to sidestep shell quoting issues.
        let mut runner_text = cmd
            .iter()
            .map(|s| shell_escape(s))
            .collect::<Vec<_>>()
            .join(" ");
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
        tokio::fs::write(&runner, format!("{}\n", runner_text)).await?;

        // Register the peer before starting tmux so the env vars point at a
        // socket path that is already known.
        let peer = self
            .registry
            .register(&session, workdir.clone(), opts.vendor)?;

        // Build the tmux command.  We run under script(1) for a transcript,
        // inject CommRails env vars (both the new ALLTERNIT_COMMRAILS_* names
        // and the legacy ALLTERNIT_RAILS_* names for one release, so old
        // executors still see them), and chain the notes sentinel if requested.
        let mut inner = format!(
            "export ALLTERNIT_AO_PANE_ID={}; export ALLTERNIT_COMMRAILS_PEER_NAME={}; export ALLTERNIT_COMMRAILS_INBOX={}; export ALLTERNIT_COMMRAILS_ROOT={}; export ALLTERNIT_RAILS_PEER_NAME={}; export ALLTERNIT_RAILS_INBOX={}; export ALLTERNIT_RAILS_ROOT={}; ",
            shell_escape(&session),
            shell_escape(&peer.name),
            shell_escape(&peer.inbox_socket.to_string_lossy()),
            shell_escape(&workdir.to_string_lossy()),
            shell_escape(&peer.name),
            shell_escape(&peer.inbox_socket.to_string_lossy()),
            shell_escape(&workdir.to_string_lossy())
        );
        if let Some(wih_id) = opts.wih {
            inner.push_str(&format!(
                "export ALLTERNIT_COMMRAILS_WIH={}; ",
                shell_escape(wih_id)
            ));
        }
        if let Some(task) = opts.task_file {
            let task_escaped = shell_escape(&task.to_string_lossy());
            inner.push_str(&format!(
                "export ALLTERNIT_COMMRAILS_TASK_FILE={0}; export ALLTERNIT_RAILS_TASK_FILE={0}; ",
                task_escaped
            ));
        }
        inner.push_str(&format!(
            "{}script -q {} /bin/sh {}",
            env_prefix,
            shell_escape(&log.to_string_lossy()),
            shell_escape(&runner.to_string_lossy())
        ));
        if let Some(sentinel) = opts.notes_sentinel {
            inner.push_str(&format!(
                "; touch {}",
                shell_escape(&sentinel.to_string_lossy())
            ));
        }

        let status = Command::new("tmux")
            .arg("new-session")
            .arg("-d")
            .arg("-s")
            .arg(&session)
            .arg("-c")
            .arg(&workdir)
            .arg(&inner)
            .status()
            .await?;
        if !status.success() {
            self.registry.unregister(&peer.peer_id).ok();
            if wt_created {
                let _ = remove_worktree(&workdir).await;
            }
            bail!("tmux new-session failed for {}", session);
        }

        // Best-effort remain-on-exit.
        let _ = Command::new("tmux")
            .arg("set-option")
            .arg("-t")
            .arg(format!("={}:", session))
            .arg("remain-on-exit")
            .arg("on")
            .status()
            .await;

        sleep(Duration::from_millis(500)).await;
        // A captured run that already wrote its exit code finished fast (the
        // session can be gone before remain-on-exit applied); that is a
        // completed run, not a failed spawn.
        let finished_fast = opts.capture.is_some_and(|c| c.exit_code.exists());
        if !finished_fast && !tmux_has_session(&session).await? {
            self.registry.unregister(&peer.peer_id).ok();
            if wt_created {
                let _ = remove_worktree(&workdir).await;
            }
            let tail = tokio::fs::read_to_string(&log).await.unwrap_or_default();
            bail!(
                "agent exited immediately — transcript tail:\n{}",
                tail.lines().rev().take(5).collect::<Vec<_>>().join("\n")
            );
        }

        Ok(SpawnResult {
            session,
            workdir,
            logfile: log,
            peer_id: peer.peer_id,
            inbox_socket: peer.inbox_socket,
        })
    }

    /// Send data to a session.  Prefers UDS if the peer socket is bound;
    /// otherwise falls back to `tmux send-keys`.
    pub async fn send(&self, slug: &str, data: &str) -> Result<()> {
        let session = format!("ao-{}", sanitize_slug(slug));
        if let Some(peer) = self.registry.resolve(&session) {
            if peer.status != PeerStatus::Dead && peer.inbox_socket.exists() {
                let envelope = PeerEnvelope::new("orchestrator", &peer.name, data);
                let receipt = send_envelope(&peer.inbox_socket, &envelope, Duration::from_secs(5))
                    .await?;
                if receipt.delivered {
                    return Ok(());
                }
            }
        }
        tmux_send_keys(&session, data).await
    }

    /// Block until the sentinel file exists, the tmux pane dies, or the timeout
    /// elapses.  Returns the matching outcome.
    pub async fn watch(
        &self,
        slug: &str,
        sentinel: &Path,
        timeout_seconds: u64,
        interval_seconds: u64,
    ) -> Result<WatchOutcome> {
        let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
        let interval = Duration::from_secs(interval_seconds.max(1));

        loop {
            if let Some(outcome) = self.poll(slug, sentinel).await {
                return Ok(outcome);
            }
            if Instant::now() >= deadline {
                return Ok(WatchOutcome::Timeout);
            }
            sleep(interval).await;
        }
    }

    /// One non-blocking watch step: `Some(Done)` when the sentinel exists,
    /// `Some(Dead)` when the pane is dead or the session gone, `None` while
    /// it is still running. Timeouts are the caller's clock.
    pub async fn poll(&self, slug: &str, sentinel: &Path) -> Option<WatchOutcome> {
        if sentinel.exists() {
            return Some(WatchOutcome::Done);
        }
        let session = format!("ao-{}", sanitize_slug(slug));
        match tmux_pane_dead(&session).await {
            Ok(false) => None,
            // Pane dead, or session gone: re-check the sentinel once, since a
            // run can write it and exit between the two checks.
            Ok(true) | Err(_) if sentinel.exists() => Some(WatchOutcome::Done),
            Ok(true) | Err(_) => Some(WatchOutcome::Dead),
        }
    }

    /// True while the session exists and its pane is still running.
    pub async fn is_alive(&self, slug: &str) -> bool {
        session_alive(slug).await
    }

    /// Return a human-readable status summary.
    pub async fn status(&self, slug: Option<&str>, lines: usize) -> Result<String> {
        if let Some(slug) = slug {
            let session = format!("ao-{}", sanitize_slug(slug));
            if !tmux_has_session(&session).await? {
                if let Some(peer) = self.registry.resolve(&session) {
                    return Ok(format!(
                        "== {} (DEAD in tmux; peer {} still registered) ==",
                        session, peer.peer_id
                    ));
                }
                bail!("no session {}", session);
            }
            let dead = tmux_pane_dead(&session).await?;
            let state = if dead { "DEAD" } else { "alive" };
            let mut out = format!("== {} ({}) — last {} lines ==\n", session, state, lines);
            let output = Command::new("tmux")
                .arg("capture-pane")
                .arg("-p")
                .arg("-t")
                .arg(format!("={}:", session))
                .arg("-S")
                .arg(format!("-{}", lines))
                .output()
                .await?;
            out.push_str(&String::from_utf8_lossy(&output.stdout));
            Ok(out)
        } else {
            let sessions = tmux_list_ao_sessions().await?;
            if sessions.is_empty() {
                return Ok("no ao-* sessions".to_string());
            }
            let mut lines = Vec::new();
            for session in sessions {
                let dead = tmux_pane_dead(&session).await.unwrap_or(true);
                let dir = tmux_pane_cwd(&session).await.unwrap_or_default();
                let state = if dead { "DEAD" } else { "alive" };
                lines.push(format!("{}  {}  {}", session, state, dir));
            }
            Ok(lines.join("\n"))
        }
    }

    /// Kill a session and unregister its peer.
    pub async fn kill(&self, slug: &str, rm_worktree: bool) -> Result<()> {
        let session = format!("ao-{}", sanitize_slug(slug));
        let mut wt_dir = None;
        if tmux_has_session(&session).await? {
            wt_dir = tmux_pane_cwd(&session).await.ok();
            let _ = Command::new("tmux")
                .arg("kill-session")
                .arg("-t")
                .arg(format!("={}:", session))
                .status()
                .await?;
        }
        self.registry.unregister(&session).ok();

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

    /// Probe the local delegation toolchain.
    pub async fn doctor(&self) -> Result<DoctorReport> {
        let mut transport_ok = true;
        for binary in ["tmux", "script", "git"] {
            if !which(binary).await {
                transport_ok = false;
            }
        }

        let mut executors = Vec::new();
        executors.push(probe_executor("kimi", "kimi", &["--yolo"], &[]).await);
        // Probe the flags the spawn gate actually launches with (see hook::gate_argv):
        // codex runs sandboxed, claude runs acceptEdits + a --settings PreToolUse hook.
        executors.push(probe_executor("codex", "codex", &["--config"], &["exec"]).await);
        executors.push(probe_executor("claude", "claude", &["--permission-mode", "--settings"], &["-p", "--permission-mode", "--settings"]).await);
        executors.push(probe_executor("agy", "agy", &["--dangerously-skip-permissions"], &[]).await);

        // Loopback UDS round-trip (Unix only).
        #[cfg(unix)]
        let delivered = {
            let loopback_peer = self
                .registry
                .register("ao-doctor-loopback", self.root_dir.clone(), "rails")?;
            let mut listener = crate::peer::PeerSocket::bind(&loopback_peer.inbox_socket).await?;
            let listen_path = loopback_peer.inbox_socket.clone();
            let handle = tokio::spawn(async move {
                listener.accept_envelope().await.ok()
            });
            let env = PeerEnvelope::new("doctor", &loopback_peer.name, "loopback");
            let delivered = send_envelope(&listen_path, &env, Duration::from_secs(2))
                .await
                .map(|r| r.delivered)
                .unwrap_or(false);
            let _ = handle.await;
            self.registry.unregister(&loopback_peer.peer_id).ok();
            delivered
        };

        let report = DoctorReport {
            transport_ok,
            executors,
        };

        let any_usable = report.executors.iter().any(|e| {
            e.installed && (e.interactive_flags_ok || e.headless_flags_ok)
        });

        println!("orchestrator doctor: transport");
        for binary in ["tmux", "script", "git"] {
            println!("  {}: {}", binary, if which(binary).await { "OK" } else { "MISSING" });
        }
        println!("orchestrator doctor: executors");
        for e in &report.executors {
            println!(
                "  {} ({}): installed={} interactive={} headless={} gate={} version={:?}",
                e.vendor,
                e.binary,
                e.installed,
                e.interactive_flags_ok,
                e.headless_flags_ok,
                hook::harness_gate(&e.binary).as_str(),
                e.version
            );
        }
        #[cfg(unix)]
        println!(
            "orchestrator doctor: uds loopback={}",
            if delivered { "OK" } else { "FAILED" }
        );
        #[cfg(not(unix))]
        println!("orchestrator doctor: uds loopback=UNSUPPORTED (http inbox polling only)");
        if !transport_ok {
            bail!("TRANSPORT BROKEN");
        }
        if !any_usable {
            bail!("NO USABLE EXECUTORS");
        }
        Ok(report)
    }
}

/// True while `ao-<slug>` exists and its pane is still running (read-only).
pub async fn session_alive(slug: &str) -> bool {
    matches!(tmux_pane_dead(&session_name(slug)).await, Ok(false))
}

/// Session-name form of a slug (`ao-<sanitized>`).
pub fn session_name(slug: &str) -> String {
    format!("ao-{}", sanitize_slug(slug))
}

fn sanitize_slug(slug: &str) -> String {
    slug.trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            ' ' => '-',
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
}

fn shell_escape(s: &str) -> String {
    // POSIX single quotes: everything inside is literal, including
    // backslashes (doubling them corrupted any argv with a `\`, e.g. an
    // ao-spawn gated line that quotes a path with `'\''`). Only `'` needs
    // closing, escaping and reopening.
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

async fn tmux_has_session(session: &str) -> Result<bool> {
    // Probe only: tmux's "can't find session" on stderr is the expected
    // answer for a fresh slug, not an error to show the caller.
    let status = Command::new("tmux")
        .arg("has-session")
        .arg("-t")
        .arg(format!("={}:", session))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;
    Ok(status.success())
}

async fn tmux_pane_dead(session: &str) -> Result<bool> {
    let output = Command::new("tmux")
        .arg("list-panes")
        .arg("-t")
        .arg(format!("={}:", session))
        .arg("-F")
        .arg("#{pane_dead}")
        .output()
        .await?;
    if !output.status.success() {
        bail!("tmux list-panes failed");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.trim() != "0")
}

async fn tmux_pane_cwd(session: &str) -> Result<String> {
    let output = Command::new("tmux")
        .arg("display-message")
        .arg("-p")
        .arg("-t")
        .arg(format!("={}:", session))
        .arg("#{pane_current_path}")
        .output()
        .await?;
    if !output.status.success() {
        bail!("tmux display-message failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn tmux_list_ao_sessions() -> Result<Vec<String>> {
    let output = Command::new("tmux")
        .arg("list-sessions")
        .arg("-F")
        .arg("#{session_name}")
        .output()
        .await?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| l.starts_with("ao-"))
        .map(|l| l.to_string())
        .collect())
}

async fn tmux_send_keys(session: &str, keys: &str) -> Result<()> {
    let status = Command::new("tmux")
        .arg("send-keys")
        .arg("-t")
        .arg(format!("={}:", session))
        .arg(keys)
        .arg("Enter")
        .status()
        .await?;
    if !status.success() {
        bail!("tmux send-keys failed for {}", session);
    }
    Ok(())
}

async fn create_worktree(repo: &Path, slug: &str) -> Result<(PathBuf, bool)> {
    let root = git_toplevel(repo).await?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
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
        .arg("worktree")
        .arg("add")
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
    let status = Command::new("git")
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(wt)
        .status()
        .await;
    if status.map(|s| s.success()).unwrap_or(false) {
        return Ok(());
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(wt)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(wt)
        .status()
        .await?;
    if !status.success() {
        bail!("could not remove worktree {}", wt.display());
    }
    Ok(())
}

fn is_ao_worktree(dir: &Path, slug: &str) -> bool {
    let suffix = format!("-ao-{}", slug);
    dir.to_string_lossy().ends_with(&suffix)
}

async fn git_toplevel(repo: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .await?;
    if !output.status.success() {
        bail!("not a git repository: {}", repo.display());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

async fn which(binary: &str) -> bool {
    Command::new("command")
        .arg("-v")
        .arg(binary)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn probe_executor(
    vendor: &str,
    binary: &str,
    interactive_flags: &[&str],
    headless_flags: &[&str],
) -> ExecutorProbe {
    let installed = which(binary).await;
    let version = if installed {
        Command::new(binary)
            .arg("--version")
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.lines().next().unwrap_or("").to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    let help_text = if installed {
        Command::new(binary)
            .arg("--help")
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let interactive_ok = interactive_flags.iter().all(|f| help_text.contains(f));
    let headless_ok = if headless_flags.is_empty() {
        false
    } else {
        headless_flags.iter().all(|f| help_text.contains(f))
    };

    let note = match vendor {
        "kimi" => "no headless: -p refuses --yolo/--auto".to_string(),
        "codex" => "--yolo no longer exposed (July 2026 CLI)".to_string(),
        _ => String::new(),
    };

    ExecutorProbe {
        vendor: vendor.to_string(),
        binary: binary.to_string(),
        installed,
        version,
        interactive_flags_ok: interactive_ok,
        headless_flags_ok: headless_ok,
        note,
    }
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
