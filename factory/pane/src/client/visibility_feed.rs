//! P5 visibility feed watcher: background thread that samples the three
//! panel feeds and posts them to the client loop.
//!
//! Cadence (fixed numbers, documented in `docs/programs/ao/AO_VISIBILITY_PEERS_NOTES.md`):
//! - engine agents (`agent.list` over the same socket contract the CLI uses):
//!   every 2s — this is also what derives waiting-on-you transitions;
//! - native catalog scan + Rails peers: re-walked every 15s and cached
//!   between ticks (home-dir walks are cheap on mtime, but there is no need
//!   to re-walk 27 harness dirs every 2s).
//!
//! Registry root for feed 3 is the ao process's current working directory
//! (overridable with `AO_PEERS_ROOT`) — the registry is local-only, so a
//! wrong root means an empty peers section.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::ClientLoopEvent;

const ENGINE_POLL_INTERVAL: Duration = Duration::from_secs(2);
const CATALOG_POLL_INTERVAL: Duration = Duration::from_secs(15);

pub(super) fn watch_visibility(
    event_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>,
    should_quit: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let client = crate::api::client::ApiClient::local();
        let mut tick = 0_u32;
        let mut cached_native: Vec<crate::ao::native::NativeSession> = Vec::new();
        let mut cached_peers: Vec<allternit_commrails::peer::Peer> = Vec::new();
        while !should_quit.load(Ordering::Acquire) {
            tick = tick.wrapping_add(1);
            let refresh_slow = tick == 1
                || tick % (CATALOG_POLL_INTERVAL.as_secs() as u32 / ENGINE_POLL_INTERVAL.as_secs() as u32) == 0;
            if refresh_slow {
                cached_native = crate::ao::native::home_dir()
                    .as_deref()
                    .map(|h| crate::ao::native::list_native_sessions(h, None, None))
                    .unwrap_or_default();
                let peers_root = crate::ao::peers::registry_root(None)
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                cached_peers = crate::ao::peers::read_registry(&peers_root).unwrap_or_default();
            }

            let sample = crate::ao::visibility::FeedSample {
                engine: crate::ao::visibility::engine_feed(&client),
                native: cached_native.clone(),
                peers: cached_peers.clone(),
            };
            if event_tx
                .blocking_send(ClientLoopEvent::VisibilityFeed(Arc::new(sample)))
                .is_err()
            {
                break;
            }
            std::thread::sleep(ENGINE_POLL_INTERVAL);
        }
    });
}
