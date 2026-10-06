//! The pane backend: the one way the engine reaches live agent terminals.
//!
//! Every spawn, send, capture and kill goes through the pane engine
//! (`factory/pane`, the Herdr fork). The engine crate can't link the pane crate
//! (the pane crate links this one), so the binary that has both
//! (`allternit-factory`) installs the pane engine's implementation at startup
//! with [`install`]. Nothing falls back to tmux or a raw child process: with no
//! backend installed, every call fails with a [`Transport`] error that says so.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Result;

/// A pane to start: one agent session, labeled `session` (`ao-<slug>`).
#[derive(Debug, Clone)]
pub struct PaneSpawn {
    /// Session label (`ao-<slug>`): the pane engine workspace label.
    pub session: String,
    pub cwd: PathBuf,
    /// The command the pane runs.
    pub argv: Vec<String>,
    /// Extra environment for the pane's process.
    pub env: BTreeMap<String, String>,
    /// Where the pane engine tees the terminal transcript.
    pub transcript: Option<PathBuf>,
}

/// A live agent pane, as the pane engine reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePane {
    pub session: String,
    pub pane_id: String,
    pub cwd: Option<String>,
    /// The pane engine's agent status (`idle`, `working`, `blocked`, `done`)
    /// when it detected an agent in the pane; `None` when it could not tell.
    pub agent_status: Option<String>,
}

/// How a text reached a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneSend {
    /// Pasted, read back from the pane twice, and submitted.
    Verified,
    /// Put on the session's mailbox (the pane was busy, gone, or the paste
    /// could not be read back). The drainer pastes it when the pane is idle.
    Queued { message_id: String, depth: usize, reason: String },
}

/// A pane's visible screen, as ANSI text, for a mirror view (Desktop's
/// terminal tiles). `revision` changes whenever the screen does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneScreen {
    pub ansi: String,
    pub revision: u64,
}

/// Whether the pane engine is up (reported by `agents ps`, never hidden).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineStatus {
    pub running: bool,
    pub error: Option<String>,
}

/// The pane engine, as the engine sees it. Calls are blocking (the pane
/// engine's API is a local socket); call them off the async runtime with
/// [`blocking`].
pub trait PaneBackend: Send + Sync {
    /// Start a pane. Fails if `session` already exists.
    fn spawn(&self, req: &PaneSpawn) -> Result<LivePane>;
    /// Every live agent pane: sessions labeled `ao-…`, plus any other pane
    /// the pane engine sees an agent in (as session `ao-pane-<paneId>`).
    fn list(&self) -> Result<Vec<LivePane>>;
    /// One session's pane, if it is live.
    fn find(&self, session: &str) -> Result<Option<LivePane>> {
        Ok(self.list()?.into_iter().find(|p| p.session == session))
    }
    /// Deliver `text` to a session: a verified paste when the pane is idle,
    /// else the mailbox. `queue_only` skips the paste attempt. `root` is the
    /// workspace whose Bus holds the mailbox; `sender` is who sent it.
    fn send(&self, root: &std::path::Path, session: &str, text: &str, sender: &str, queue_only: bool) -> Result<PaneSend>;
    /// The last `lines` lines of the pane's screen.
    fn capture(&self, session: &str, lines: u32) -> Result<String>;
    /// Close the session's pane.
    fn kill(&self, session: &str) -> Result<()>;
    /// The session's visible screen with its colors (ANSI), for a mirror view.
    fn screen(&self, session: &str) -> Result<PaneScreen> {
        Err(anyhow::anyhow!("this pane engine cannot read screens ({session})"))
    }
    /// Type into the session's pane as a person at the keyboard would: `text`
    /// is written as-is, `keys` are named keys (`Enter`, `C-c`, `Up`, …).
    /// Nothing is recorded: this is the terminal, not a send.
    fn input(&self, session: &str, _text: &str, _keys: &[String]) -> Result<()> {
        Err(anyhow::anyhow!("this pane engine cannot take input ({session})"))
    }
    /// The session's queued messages, oldest first (see [`PaneBackend::send`]).
    fn mailbox(&self, _root: &std::path::Path, session: &str) -> Result<Vec<Queued>> {
        Err(anyhow::anyhow!("this pane engine has no mailbox ({session})"))
    }
    /// Type `text` into the session's pane if it is idle, verify it landed,
    /// then submit it. `Ok(false)`: the pane is gone, busy, or the paste could
    /// not be read back (nothing was submitted).
    fn deliver(&self, session: &str, _text: &str) -> Result<bool> {
        Err(anyhow::anyhow!("this pane engine cannot deliver queued messages ({session})"))
    }
    /// Mark a queued message delivered. Only after a verified [`deliver`].
    ///
    /// [`deliver`]: PaneBackend::deliver
    fn settle(&self, _root: &std::path::Path, id: &str) -> Result<()> {
        Err(anyhow::anyhow!("this pane engine has no mailbox (message {id})"))
    }
    /// Whether the pane engine is running (no panes can be live when not).
    fn status(&self) -> EngineStatus {
        EngineStatus { running: true, error: None }
    }
}

/// One message waiting in a session's mailbox.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Queued {
    pub id: String,
    pub text: String,
}

/// What [`drain`] did with one queued message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drained {
    pub message_id: String,
    /// True: typed into the pane, verified, and settled. False: left queued.
    pub delivered: bool,
}

/// Deliver a session's queued messages, oldest first, through the same
/// verified paste as a send. A message is settled only after its delivery is
/// verified; the first one that can't be delivered stays queued and stops
/// the drain (order is kept). Without `all`, at most one message is tried.
/// Blocking: call it with [`blocking`] from async code.
pub fn drain(root: &std::path::Path, session: &str, all: bool) -> Result<Vec<Drained>> {
    let pane = backend()?;
    let mut out = Vec::new();
    for msg in pane.mailbox(root, session)? {
        if !pane.deliver(session, &msg.text)? {
            out.push(Drained { message_id: msg.id, delivered: false });
            break;
        }
        pane.settle(root, &msg.id)?;
        out.push(Drained { message_id: msg.id, delivered: true });
        if !all {
            break;
        }
    }
    Ok(out)
}

/// The pane engine is unreachable, or no pane engine is linked. Maps to exit
/// code 3 / HTTP 502 `transport`.
#[derive(Debug, Clone)]
pub struct Transport(pub String);

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Transport {}

/// True when `err` (or its cause chain) is a [`Transport`] failure.
pub fn is_transport(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<Transport>().is_some())
}

static BACKEND: RwLock<Option<Arc<dyn PaneBackend>>> = RwLock::new(None);

/// Install the pane engine. The binary calls this once at startup; tests
/// install a fake. A later call replaces the earlier one.
pub fn install(backend: Arc<dyn PaneBackend>) {
    *BACKEND.write().unwrap_or_else(|e| e.into_inner()) = Some(backend);
}

/// The installed pane engine, or a [`Transport`] error naming the gap.
pub fn backend() -> Result<Arc<dyn PaneBackend>> {
    BACKEND
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| {
            anyhow::Error::new(Transport(
                "no pane engine in this process (agent panes need the allternit-factory binary)".to_string(),
            ))
        })
}

/// Whether a pane engine is installed.
pub fn installed() -> bool {
    BACKEND.read().map(|b| b.is_some()).unwrap_or(false)
}

/// Run a blocking backend call from async code. It runs on its own OS thread,
/// outside any tokio context, because pane mailbox calls start their own
/// short-lived runtime.
pub async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.await
        .map_err(|_| anyhow::anyhow!("pane backend call panicked"))?
}
