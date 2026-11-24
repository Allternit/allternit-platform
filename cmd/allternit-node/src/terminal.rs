//! PTY-backed terminal sessions served in-process over the relay — the same
//! HTTP contract as `allternit-api/src/terminal_routes.rs`
//! (create/input/close/resize/stream) so fabric terminal sessions work with
//! the desktop app fully quit.
//!
//! portable-pty is synchronous; the open and the output reader run on
//! blocking threads, and output is fanned out to stream subscribers over a
//! broadcast channel. Each session also keeps a bounded in-memory scrollback
//! that new stream subscribers replay first (the mux-backed gateway persists
//! scrollback across restarts; the daemon only promises in-memory replay).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use portable_pty::{native_pty_system, Child, MasterPty, PtySize};
use tokio::sync::broadcast;

const SCROLLBACK_CAP: usize = 64 * 1024;
const OUTPUT_CHANNEL_CAP: usize = 256;

struct TerminalSession {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    output: broadcast::Sender<String>,
    scrollback: Arc<Mutex<String>>,
    child: Arc<Mutex<Box<dyn Child + Send + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    created: Instant,
}

#[derive(Default)]
pub struct TerminalStore {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CreatedSession {
    pub session_id: String,
}

/// Kill a session's PTY child and reap it (wait), so it never outlives the
/// daemon as a zombie.
fn kill_and_reap(session: &TerminalSession) {
    let _ = session.child.lock().unwrap().kill();
    let _ = session.child.lock().unwrap().wait();
}

impl TerminalStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(
        &self,
        shell: &str,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<CreatedSession> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open pty")?;
        let mut command = portable_pty::CommandBuilder::new(shell);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        // A coding agent's session markers, inherited when the node was started
        // from inside an agent's shell. A `claude` run in this terminal would
        // otherwise act as that session's child and stop saving its transcript.
        // Same list as factory/pane/src/pane.rs INHERITED_AGENT_SESSION_ENV.
        for key in [
            "CODEX_THREAD_ID",
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_SESSION_ATTENDED",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_EXECPATH",
            "CLAUDE_PID",
            "AI_AGENT",
        ] {
            command.env_remove(key);
        }
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .context("spawn shell in pty")?;
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .context("pty writer")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("pty reader")?;

        let (output, _) = broadcast::channel(OUTPUT_CHANNEL_CAP);
        let scrollback = Arc::new(Mutex::new(String::new()));
        let session = Arc::new(TerminalSession {
            writer: Arc::new(Mutex::new(writer)),
            output: output.clone(),
            scrollback: scrollback.clone(),
            child: Arc::new(Mutex::new(child)),
            master: Arc::new(Mutex::new(pair.master)),
            created: Instant::now(),
        });
        let session_id = uuid::Uuid::new_v4().to_string();
        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.clone(), session.clone());

        // Blocking reader pump: PTY bytes → broadcast + capped scrollback.
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                        {
                            let mut scroll = scrollback.lock().unwrap();
                            scroll.push_str(&chunk);
                            if scroll.len() > SCROLLBACK_CAP {
                                let overflow = scroll.len() - SCROLLBACK_CAP;
                                scroll.drain(..overflow);
                            }
                        }
                        // Subscribers may be gone; lagged receivers resync from
                        // scrollback on the next stream attach.
                        let _ = output.send(chunk);
                    }
                }
            }
        });

        Ok(CreatedSession { session_id })
    }

    fn get(&self, session_id: &str) -> Option<Arc<TerminalSession>> {
        self.sessions.lock().unwrap().get(session_id).cloned()
    }

    pub fn input(&self, session_id: &str, content: &str) -> anyhow::Result<()> {
        let session = self
            .get(session_id)
            .ok_or_else(|| anyhow::anyhow!("terminal session '{session_id}' not found"))?;
        session
            .writer
            .lock()
            .unwrap()
            .write_all(content.as_bytes())
            .context("write to pty")?;
        Ok(())
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        let session = self
            .get(session_id)
            .ok_or_else(|| anyhow::anyhow!("terminal session '{session_id}' not found"))?;
        session
            .master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize pty")?;
        Ok(())
    }

    pub fn close(&self, session_id: &str) -> anyhow::Result<()> {
        let session = self.sessions.lock().unwrap().remove(session_id);
        if let Some(session) = session {
            kill_and_reap(&session);
        }
        Ok(())
    }

    /// Kill and reap every tracked session. Called once at process shutdown
    /// (SIGTERM/SIGINT) so the service never leaks PTY children as zombies
    /// across restarts.
    pub fn shutdown_all(&self) {
        let sessions: Vec<(String, Arc<TerminalSession>)> =
            self.sessions.lock().unwrap().drain().collect();
        for (session_id, session) in sessions {
            kill_and_reap(&session);
            tracing::info!(%session_id, "terminal session shut down");
        }
    }

    /// Snapshot of the scrollback a new subscriber starts with.
    pub fn scrollback(&self, session_id: &str) -> Option<String> {
        self.get(session_id)
            .map(|session| session.scrollback.lock().unwrap().clone())
    }

    /// Live output receiver for a session (post-scrollback stream).
    pub fn subscribe(&self, session_id: &str) -> Option<broadcast::Receiver<String>> {
        self.get(session_id).map(|session| session.output.subscribe())
    }

    /// True while the child process is still running.
    pub fn alive(&self, session_id: &str) -> bool {
        self.get(session_id)
            .map(|session| matches!(session.child.lock().unwrap().try_wait(), Ok(None)))
            .unwrap_or(false)
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    #[allow(dead_code)]
    pub fn uptime(&self, session_id: &str) -> Option<Duration> {
        self.get(session_id).map(|session| session.created.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_lifecycle_echoes_input() {
        let store = TerminalStore::new();
        let shell = if cfg!(target_os = "windows") {
            "cmd.exe"
        } else {
            "/bin/sh"
        };
        let created = store.create(shell, None, 80, 24).unwrap();
        let mut received = store
            .subscribe(&created.session_id)
            .expect("subscriber");

        // Feed a command whose output proves the PTY round-trip. Subscribe
        // first: broadcast is live-only (scrollback replay covers stream
        // attaches, not pre-existing receivers).
        let marker = format!("allt-{}", uuid::Uuid::new_v4().simple());
        let script = if cfg!(target_os = "windows") {
            format!("echo {marker}\r\n")
        } else {
            format!("echo {marker}\n")
        };
        store.input(&created.session_id, &script).unwrap();
        let mut found = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline && !found.contains(&marker) {
            match tokio::time::timeout(Duration::from_millis(500), received.recv()).await {
                Ok(Ok(chunk)) => found.push_str(&chunk),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                _ => continue,
            }
        }
        store.close(&created.session_id).unwrap();
        assert!(
            found.contains(&marker),
            "pty output must echo the marker, got: {found:?}"
        );
        assert_eq!(store.session_count(), 0);
        assert!(!store.alive(&created.session_id));
    }

    #[tokio::test]
    async fn shutdown_all_kills_and_reaps_live_sessions() {
        let store = TerminalStore::new();
        let shell = if cfg!(target_os = "windows") {
            "cmd.exe"
        } else {
            "/bin/sh"
        };
        let first = store.create(shell, None, 80, 24).unwrap();
        let second = store.create(shell, None, 80, 24).unwrap();
        assert_eq!(store.session_count(), 2);
        assert!(store.alive(&first.session_id));
        assert!(store.alive(&second.session_id));

        store.shutdown_all();

        assert_eq!(store.session_count(), 0);
        assert!(!store.alive(&first.session_id));
        assert!(!store.alive(&second.session_id));
    }
}
