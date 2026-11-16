//! Watches the sync root for local edits, new files and pin / free-up-space
//! choices (attribute changes), and hands each settled path to
//! [`Provider::sync_path`]. Deletes and renames come through cfapi callbacks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use serde_json::json;

use crate::provider::{Provider, SyncOutcome};

/// A path is handled once it has been quiet this long (apps save in bursts).
pub const SETTLE: Duration = Duration::from_secs(2);

pub struct LocalWatcher {
    _watcher: notify::RecommendedWatcher,
    thread: JoinHandle<()>,
}

impl LocalWatcher {
    pub fn start(provider: Arc<Provider>) -> notify::Result<Self> {
        let (tx, rx) = channel::<PathBuf>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    let _ = tx.send(p);
                }
            }
        })?;
        watcher.watch(&provider.root, RecursiveMode::Recursive)?;
        let thread = std::thread::spawn(move || {
            let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
            loop {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(p) => {
                        pending.insert(p, Instant::now());
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let mut ready: Vec<PathBuf> =
                    pending.iter().filter(|(_, t)| t.elapsed() >= SETTLE).map(|(p, _)| p.clone()).collect();
                // Parents before children: a new folder must exist on the server first.
                ready.sort_by_key(|p| p.components().count());
                for p in ready {
                    pending.remove(&p);
                    match provider.sync_path(&p) {
                        Ok(SyncOutcome::Skipped | SyncOutcome::InSync) => {}
                        Ok(done) => crate::emit(&json!({"type": "synced", "path": p.display().to_string(), "result": format!("{done:?}")})),
                        Err(e) => {
                            crate::emit(&json!({"type": "sync_failed", "path": p.display().to_string(), "error": e}));
                            // Locked by the editing app, or the computer was asleep and the
                            // user said not now: try again later.
                            pending.insert(p, Instant::now() + Duration::from_secs(30));
                        }
                    }
                }
            }
        });
        Ok(LocalWatcher { _watcher: watcher, thread })
    }

    pub fn join(self) {
        drop(self._watcher);
        let _ = self.thread.join();
    }
}
