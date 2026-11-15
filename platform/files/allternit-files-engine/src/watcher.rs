//! Filesystem watcher feeding the index. Events are batched for a short quiet period
//! and handed to [`Drive::reconcile_paths`], so a burst (an app saving through a temp
//! file, a folder move) becomes one reconcile and renames keep their item ID.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::drive::Drive;
use crate::error::{FilesError, Result};
use crate::naming::TEMP_PREFIX;

/// Quiet period before a batch is reconciled.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

enum Msg {
    Paths(Vec<PathBuf>),
    Stop,
}

/// Watches every root of a drive. Dropping it stops the watcher thread.
pub struct DriveWatcher {
    watcher: Mutex<RecommendedWatcher>,
    tx: mpsc::Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl DriveWatcher {
    /// Starts watching all current roots. `on_error` gets reconcile failures (log them).
    pub fn start(drive: Arc<Drive>, on_error: impl Fn(FilesError) + Send + 'static) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let ev_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let paths: Vec<PathBuf> = ev
                    .paths
                    .into_iter()
                    .filter(|p| !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(TEMP_PREFIX)))
                    .collect();
                if !paths.is_empty() {
                    let _ = ev_tx.send(Msg::Paths(paths));
                }
            }
        })
        .map_err(|e| FilesError::Internal(format!("watcher: {e}")))?;
        for root in drive.roots()? {
            watcher
                .watch(&root.local_path, RecursiveMode::Recursive)
                .map_err(|e| FilesError::Internal(format!("watch {}: {e}", root.local_path.display())))?;
        }
        let thread = std::thread::Builder::new().name("files-watcher".into()).spawn(move || {
            let mut batch: HashSet<PathBuf> = HashSet::new();
            let mut deadline: Option<Instant> = None;
            loop {
                let msg = match deadline {
                    Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match msg {
                    Ok(Msg::Paths(p)) => {
                        batch.extend(p);
                        deadline = Some(Instant::now() + DEBOUNCE);
                    }
                    Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {
                        let paths: Vec<PathBuf> = batch.drain().collect();
                        deadline = None;
                        if let Err(e) = drive.reconcile_paths(&paths) {
                            on_error(e);
                        }
                    }
                }
            }
        })?;
        Ok(Self { watcher: Mutex::new(watcher), tx, thread: Some(thread) })
    }

    /// Starts watching a root added after `start`.
    pub fn watch_root(&self, path: &std::path::Path) -> Result<()> {
        self.watcher
            .lock()
            .expect("watcher lock")
            .watch(path, RecursiveMode::Recursive)
            .map_err(|e| FilesError::Internal(format!("watch {}: {e}", path.display())))
    }

    pub fn unwatch_root(&self, path: &std::path::Path) -> Result<()> {
        self.watcher
            .lock()
            .expect("watcher lock")
            .unwatch(path)
            .map_err(|e| FilesError::Internal(format!("unwatch {}: {e}", path.display())))
    }
}

impl Drop for DriveWatcher {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
