//! PTY transcript tee — the one intentional engine diff beyond the P0 gut list.
//!
//! `ao spawn` requests a per-pane transcript by putting the log path in the
//! pane launch env under [`TRANSCRIPT_ENV_VAR`]. The launch path opens the file
//! here and the pane's `on_read` closure writes every raw PTY output byte
//! (before parsing/filtering, so the log matches `script -q` semantics:
//! byte-0 complete, ANSI included, scrollback-clear sequences included).
//!
//! The marker env var is stripped from the child process environment by
//! `apply_pane_launch_env` in `src/pane.rs`; it is spawn-time configuration,
//! not something the agent should see.
//!
//! Writes are synchronous like `script(1)`: a slow disk backpressures the
//! pane's output path instead of dropping bytes. On write error the tee
//! disables itself and logs once — the pane keeps working without it.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

/// Launch-env key carrying the transcript log path for a pane.
pub(crate) const TRANSCRIPT_ENV_VAR: &str = "HERDR_AO_TRANSCRIPT";

/// Append-only transcript writer for one pane's raw PTY output.
pub(crate) struct TranscriptTee {
    file: File,
    path: PathBuf,
    disabled: bool,
}

impl TranscriptTee {
    pub(crate) fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file,
            path,
            disabled: false,
        })
    }

    /// Writes raw output bytes. Errors disable the tee permanently (logged
    /// once) so a full disk never kills the pane.
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        if self.disabled || bytes.is_empty() {
            return;
        }
        if let Err(err) = self.file.write_all(bytes) {
            self.disabled = true;
            tracing::warn!(
                path = %self.path.display(),
                err = %err,
                "ao transcript tee disabled after write error"
            );
        }
    }
}

/// Opens a tee when the launch env carries a transcript path. Open failures
/// are logged and treated as "no tee" so spawn still succeeds; `ao spawn`
/// verifies the log separately for its instant-exit tail.
pub(crate) fn tee_from_launch_env(extra: &[(String, String)]) -> Option<TranscriptTee> {
    let path = extra
        .iter()
        .find(|(key, _)| key == TRANSCRIPT_ENV_VAR)
        .map(|(_, value)| PathBuf::from(value))?;
    match TranscriptTee::open(path.clone()) {
        Ok(tee) => Some(tee),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                err = %err,
                "ao transcript tee failed to open"
            );
            None
        }
    }
}
