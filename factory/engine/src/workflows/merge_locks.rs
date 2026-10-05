//! Merge locks for the Rails CLI.
//!
//! Merge locks are an exclusive lock over a conflict domain that prevents
//! concurrent conflicting changes.
//! They are used during conflict-prone work such as branch merges, schema
//! changes, or coordinated deployments.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};

/// Default directory for merge locks, relative to workspace root.
pub const MERGE_LOCK_DIR: &str = ".allternit/rails/merge_locks";

/// Default lock TTL if not specified.
pub const DEFAULT_LOCK_TTL_SECONDS: i64 = 3600; // 1 hour

/// An exclusive merge lock over a conflict domain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeLock {
    pub id: String,
    /// Conflict domain, e.g. `branch:main`, `path:src/db/schema.sql`.
    pub domain: String,
    pub owner: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
}

impl MergeLock {
    /// Whether the lock is currently active at the given time.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.released_at.is_none() && now < self.expires_at
    }
}

/// Store for merge locks.
pub struct MergeLockStore {
    locks_dir: PathBuf,
}

impl MergeLockStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let locks_dir = root.join(MERGE_LOCK_DIR);
        ensure_dir(&locks_dir)?;
        Ok(Self { locks_dir })
    }

    /// Acquire a lock on a domain.
    ///
    /// Returns `Ok(lock)` if acquired. Returns `Err` if an active lock already
    /// exists for the domain.
    pub fn acquire(
        &self,
        domain: impl Into<String>,
        owner: impl Into<String>,
        ttl_seconds: i64,
    ) -> Result<MergeLock> {
        let domain = domain.into();
        let owner = owner.into();

        if let Some(existing) = self.active_for_domain(&domain)? {
            anyhow::bail!(
                "domain {domain} is already locked by {} until {}",
                existing.owner,
                existing.expires_at
            );
        }

        let id = generate_lock_id();
        let acquired_at = Utc::now();
        let expires_at = acquired_at + Duration::seconds(ttl_seconds);
        let lock = MergeLock {
            id: id.clone(),
            domain,
            owner,
            acquired_at,
            expires_at,
            released_at: None,
        };
        self.write(&lock)?;
        Ok(lock)
    }

    /// Release a lock by ID.
    pub fn release(&self, id: &str) -> Result<MergeLock> {
        let mut lock = self
            .get(id)?
            .with_context(|| format!("merge lock {id} not found"))?;
        lock.released_at = Some(Utc::now());
        self.write(&lock)?;
        Ok(lock)
    }

    /// Get a lock by ID.
    pub fn get(&self, id: &str) -> Result<Option<MergeLock>> {
        read_json(&self.path(id)).with_context(|| format!("failed to read merge lock {id}"))
    }

    /// Return the active lock for a domain, if any.
    pub fn active_for_domain(&self, domain: &str) -> Result<Option<MergeLock>> {
        let now = Utc::now();
        for entry in std::fs::read_dir(&self.locks_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(lock) = read_json::<MergeLock>(&entry.path())? {
                    if lock.domain == domain && lock.is_active(now) {
                        return Ok(Some(lock));
                    }
                }
            }
        }
        Ok(None)
    }

    /// List locks, optionally only active ones.
    pub fn list(&self, active_only: bool) -> Result<Vec<MergeLock>> {
        let now = Utc::now();
        let mut locks = Vec::new();
        for entry in std::fs::read_dir(&self.locks_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(lock) = read_json::<MergeLock>(&entry.path())? {
                    if !active_only || lock.is_active(now) {
                        locks.push(lock);
                    }
                }
            }
        }
        locks.sort_by(|a, b| b.acquired_at.cmp(&a.acquired_at));
        Ok(locks)
    }

    /// Clean up expired and released locks.
    pub fn gc(&self) -> Result<usize> {
        let now = Utc::now();
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.locks_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(lock) = read_json::<MergeLock>(&entry.path())? {
                    if lock.released_at.is_some() || !lock.is_active(now) {
                        std::fs::remove_file(entry.path())?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.locks_dir.join(format!("{}.json", id))
    }

    fn write(&self, lock: &MergeLock) -> Result<()> {
        let path = self.path(&lock.id);
        write_json_atomic(&path, lock)
            .with_context(|| format!("failed to write merge lock {path:?}"))
    }
}

fn generate_lock_id() -> String {
    let mut nonce = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut nonce);
    format!("lock-{}-{}", Utc::now().timestamp_millis(), hex::encode(nonce))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquire_and_release() {
        let tmp = TempDir::new().unwrap();
        let store = MergeLockStore::new(tmp.path()).unwrap();
        let lock = store.acquire("branch:main", "ci", 3600).unwrap();
        assert!(lock.is_active(Utc::now()));

        let err = store.acquire("branch:main", "other", 3600).unwrap_err();
        assert!(err.to_string().contains("already locked"));

        store.release(&lock.id).unwrap();
        assert!(store.acquire("branch:main", "other", 3600).is_ok());
    }

    #[test]
    fn expired_locks_can_be_reacquired() {
        let tmp = TempDir::new().unwrap();
        let store = MergeLockStore::new(tmp.path()).unwrap();
        store.acquire("branch:main", "ci", -1).unwrap();
        assert!(store.acquire("branch:main", "other", 3600).is_ok());
    }

    #[test]
    fn gc_removes_stale_locks() {
        let tmp = TempDir::new().unwrap();
        let store = MergeLockStore::new(tmp.path()).unwrap();
        let lock = store.acquire("x", "ci", -1).unwrap();
        store.release(&lock.id).unwrap();

        let removed = store.gc().unwrap();
        assert!(removed >= 1);
        assert!(store.list(true).unwrap().is_empty());
    }
}
