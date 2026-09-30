//! Sweep lock: an OS advisory lock (`flock` on unix, `LockFileEx` on
//! Windows, via `std::fs::File::try_lock`) on `.allternit/rails/wakes/sweep.lock`.
//! The OS releases it when the holder exits or crashes, so there is no stale
//! lease to steal. A second sweep that cannot take it returns immediately.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::io::ensure_dir;

pub const SWEEP_LOCK_PATH: &str = ".allternit/rails/wakes/sweep.lock";

pub struct SweepLock {
    file: File,
    path: PathBuf,
}

impl SweepLock {
    /// Try to take the sweep lock; `Ok(None)` when another sweep holds it.
    pub fn try_acquire(root: &Path) -> Result<Option<Self>> {
        let path = root.join(SWEEP_LOCK_PATH);
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {
                let _ = file.set_len(0);
                let _ = writeln!(
                    file,
                    "pid={} at={}",
                    std::process::id(),
                    chrono::Utc::now().to_rfc3339()
                );
                Ok(Some(Self { file, path }))
            }
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => {
                Err(e).with_context(|| format!("locking {}", path.display()))
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SweepLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_refused_until_release() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = SweepLock::try_acquire(tmp.path()).unwrap();
        assert!(a.is_some());
        assert!(SweepLock::try_acquire(tmp.path()).unwrap().is_none());
        drop(a);
        assert!(SweepLock::try_acquire(tmp.path()).unwrap().is_some());
    }
}
