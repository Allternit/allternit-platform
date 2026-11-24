//! Which backend serves `/terminal/*`: the Factory pane engine when one
//! answers on this computer, else the in-process PTYs in `terminal.rs`.
//!
//! - **Pane engine** (`allternit-factory pane server`, a systemd unit on cloud
//!   images; Allternit Desktop's engine on a Mac): terminals are engine
//!   terminals served through `allternit-terminal-service`, the same code and
//!   JSON as allternit-api on Desktop. They survive a node restart, and a
//!   stream re-attach replays the scrollback.
//! - **Fallback** (Orgo, headless and older images have no engine): today's
//!   portable-pty terminals. They die with the node.
//!
//! The node never starts the engine (its service manager or Desktop owns it).
//! It probes the engine's socket paths at startup and, while none answered,
//! again on a request at most every 30 s. A probe is a real
//! `factory.terminal.list` call, so an engine too old to serve terminals counts
//! as missing. After an engine call answers 503 (the engine went away) the
//! next request probes again.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use allternit_terminal_service::TerminalService;
use serde_json::json;
use tokio::sync::Mutex;

/// How long a failed probe is trusted before the next request probes again.
pub const RECHECK_AFTER: Duration = Duration::from_secs(30);

/// Budget for one socket's probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The "binary" the node hands `TerminalService` for `pane tty ensure`, which
/// the service runs when its socket stops answering. It doesn't exist, so that
/// call fails at once (503) and the node never starts an engine itself.
const NO_ENGINE_START: &str = "allternit-node-does-not-start-the-pane-engine";

/// The engine's session folder under a config dir (`HERDR_SESSION=ao`, the
/// Factory's agent session; release builds use the `ao` app dir).
fn session_socket(config_dir: &Path) -> PathBuf {
    config_dir.join("ao").join("sessions").join("ao").join("herdr.sock")
}

/// The sockets to try, in order: the configured one alone when set, else
/// `$ALLTERNIT_FACTORY_PANE_SOCKET`, `$HERDR_SOCKET_PATH`, the engine's
/// default under `$XDG_CONFIG_HOME` / `%APPDATA%` / `~/.config`, and on Unix
/// root's (the cloud image's unit runs the engine as root).
pub fn socket_candidates(configured: Option<&Path>) -> Vec<PathBuf> {
    if let Some(path) = configured {
        return vec![path.to_path_buf()];
    }
    let var = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut out = Vec::new();
    out.extend(var("ALLTERNIT_FACTORY_PANE_SOCKET"));
    out.extend(var("HERDR_SOCKET_PATH"));
    out.extend(var("XDG_CONFIG_HOME").map(|dir| session_socket(&dir)));
    #[cfg(windows)]
    out.extend(var("APPDATA").map(|dir| session_socket(&dir)));
    out.extend(var("HOME").map(|home| session_socket(&home.join(".config"))));
    #[cfg(unix)]
    out.push(session_socket(Path::new("/root/.config")));
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// What the node knows about the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// Not probed yet, or the engine stopped answering.
    Unknown,
    /// The engine answered at this socket.
    Pane(PathBuf),
    /// No engine answered at this time.
    Missing(Instant),
}

/// Probe before serving? Always when unknown, never while an engine is in
/// use, and for a missing engine once [`RECHECK_AFTER`] has passed.
pub fn needs_probe(state: &EngineState, now: Instant) -> bool {
    match state {
        EngineState::Unknown => true,
        EngineState::Pane(_) => false,
        EngineState::Missing(checked) => now.saturating_duration_since(*checked) >= RECHECK_AFTER,
    }
}

/// The state after a probe that found `socket` (or nothing) at `now`.
pub fn after_probe(socket: Option<PathBuf>, now: Instant) -> EngineState {
    match socket {
        Some(socket) => EngineState::Pane(socket),
        None => EngineState::Missing(now),
    }
}

/// A service on `socket` that never starts the engine.
fn service_on(socket: &Path) -> TerminalService {
    TerminalService::with_socket(socket.to_path_buf(), PathBuf::from(NO_ENGINE_START))
}

/// The first candidate whose engine answers `factory.terminal.list`.
async fn probe(candidates: &[PathBuf]) -> Option<PathBuf> {
    for socket in candidates {
        if !socket.exists() {
            continue;
        }
        let answered = tokio::time::timeout(
            PROBE_TIMEOUT,
            service_on(socket).request("factory.terminal.list", json!({})),
        )
        .await;
        match answered {
            Ok(Ok(_)) => return Some(socket.clone()),
            Ok(Err(err)) => tracing::debug!(socket = %socket.display(), err = %err.message(), "pane engine probe failed"),
            Err(_) => tracing::debug!(socket = %socket.display(), "pane engine probe timed out"),
        }
    }
    None
}

struct Inner {
    state: EngineState,
    /// Kept across re-probes of the same socket: it holds the titles and
    /// commands of the terminals it created.
    service: Option<(PathBuf, TerminalService)>,
}

/// The node's view of the pane engine (one per daemon).
pub struct EngineSlot {
    configured: Option<PathBuf>,
    inner: Mutex<Inner>,
}

impl EngineSlot {
    /// `configured`: `NodeConfig::pane_socket` (only that socket is probed).
    pub fn new(configured: Option<PathBuf>) -> Self {
        Self {
            configured,
            inner: Mutex::new(Inner {
                state: EngineState::Unknown,
                service: None,
            }),
        }
    }

    /// The engine's terminal service, probing first when due. `None`: no
    /// engine, serve the PTY fallback.
    pub async fn get(&self) -> Option<TerminalService> {
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        if needs_probe(&inner.state, now) {
            let found = probe(&socket_candidates(self.configured.as_deref())).await;
            let was = std::mem::replace(&mut inner.state, after_probe(found.clone(), now));
            match &found {
                Some(socket) => {
                    if inner.service.as_ref().map_or(true, |(path, _)| path != socket) {
                        inner.service = Some((socket.clone(), service_on(socket)));
                    }
                    tracing::info!(socket = %socket.display(), "terminals: served by the Factory pane engine");
                }
                None if !matches!(was, EngineState::Missing(_)) => {
                    tracing::info!("terminals: no Factory pane engine answers; using in-process PTYs (they end when allternit-node restarts)");
                }
                None => {}
            }
        }
        match &inner.state {
            EngineState::Pane(_) => inner.service.as_ref().map(|(_, service)| service.clone()),
            _ => None,
        }
    }

    /// The engine stopped answering: probe again on the next request.
    pub async fn forget(&self) {
        let mut inner = self.inner.lock().await;
        if matches!(inner.state, EngineState::Pane(_)) {
            tracing::warn!("terminals: the Factory pane engine stopped answering; probing again");
            inner.state = EngineState::Unknown;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answering_engine_is_used_without_probing_again() {
        let now = Instant::now();
        let state = after_probe(Some(PathBuf::from("/root/.config/ao/sessions/ao/herdr.sock")), now);
        assert_eq!(state, EngineState::Pane(PathBuf::from("/root/.config/ao/sessions/ao/herdr.sock")));
        assert!(!needs_probe(&state, now + Duration::from_secs(3600)));
    }

    #[test]
    fn a_missing_engine_is_cached_for_30_seconds_then_probed_again() {
        let checked = Instant::now();
        let state = after_probe(None, checked);
        assert_eq!(state, EngineState::Missing(checked));
        assert!(!needs_probe(&state, checked));
        assert!(!needs_probe(&state, checked + Duration::from_secs(29)));
        assert!(needs_probe(&state, checked + RECHECK_AFTER));
    }

    #[test]
    fn an_unknown_engine_is_probed() {
        assert!(needs_probe(&EngineState::Unknown, Instant::now()));
    }

    #[test]
    fn a_configured_socket_is_the_only_candidate() {
        let path = Path::new("/run/allternit/pane.sock");
        assert_eq!(socket_candidates(Some(path)), vec![path.to_path_buf()]);
    }

    #[tokio::test]
    async fn no_socket_means_the_pty_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let slot = EngineSlot::new(Some(dir.path().join("none.sock")));
        assert!(slot.get().await.is_none());
        assert!(matches!(slot.inner.lock().await.state, EngineState::Missing(_)));
    }
}
