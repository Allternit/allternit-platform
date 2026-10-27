//! Tree leases: `<factory home>/trees/leases.json`, read and written under an
//! exclusive lock (the same pattern as the session registry).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{Owner, SeedInfo, TreeState};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub id: String,
    pub repo: String,
    pub repo_path: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    /// The origin/main commit the tree started from.
    pub base: String,
    pub owner: Owner,
    pub state: TreeState,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub seed: Option<SeedInfo>,
    /// Made by `new --once` (a ledger commit, a one-off push to main).
    #[serde(default)]
    pub once: bool,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub size_at: Option<String>,
    /// The next checkpoint number is `checkpoints + 1`.
    #[serde(default)]
    pub checkpoints: u32,
    #[serde(default)]
    pub last_checkpoint_at: Option<String>,
    /// Tree object of the last checkpoint, so an unchanged tree is not saved twice.
    #[serde(default)]
    pub last_checkpoint_tree: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct LeaseFile {
    #[serde(default)]
    trees: BTreeMap<String, Lease>,
}

#[derive(Clone, Debug)]
pub struct LeaseStore {
    path: PathBuf,
}

impl LeaseStore {
    pub fn open_default() -> Self {
        Self { path: crate::agents::registry::factory_home().join("trees/leases.json") }
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn list(&self) -> Result<Vec<Lease>> {
        Ok(self.read()?.trees.into_values().collect())
    }

    pub fn get(&self, id: &str) -> Result<Option<Lease>> {
        Ok(self.read()?.trees.remove(id))
    }

    /// The lease whose tree contains `path` (deepest match wins).
    pub fn find_by_path(&self, path: &Path) -> Result<Option<Lease>> {
        let path = canonical(path);
        Ok(self
            .list()?
            .into_iter()
            .filter(|l| path.starts_with(canonical(&l.path)))
            .max_by_key(|l| l.path.as_os_str().len()))
    }

    pub fn put(&self, lease: Lease) -> Result<()> {
        self.update(|f| {
            f.insert(lease.id.clone(), lease);
        })
    }

    pub fn remove(&self, id: &str) -> Result<Option<Lease>> {
        self.update(|f| f.remove(id))
    }

    /// Change one lease under the lock; `None` when it does not exist.
    pub fn modify<T>(&self, id: &str, f: impl FnOnce(&mut Lease) -> T) -> Result<Option<T>> {
        self.update(|trees| {
            trees.get_mut(id).map(|l| {
                let out = f(l);
                l.updated_at = super::now();
                out
            })
        })
    }

    fn update<T>(&self, f: impl FnOnce(&mut BTreeMap<String, Lease>) -> T) -> Result<T> {
        self.with_lock(|| {
            let mut file = self.read()?;
            let out = f(&mut file.trees);
            self.write(&file)?;
            Ok(out)
        })
    }

    fn read(&self) -> Result<LeaseFile> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes.is_empty() => Ok(LeaseFile::default()),
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LeaseFile::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn write(&self, file: &LeaseFile) -> Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(file)?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("replacing {}", self.path.display()))?;
        Ok(())
    }

    fn with_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let lock_path = self.path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening {}", lock_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: flock on a file descriptor we own for the call's duration.
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
                anyhow::bail!("locking {}: {}", lock_path.display(), std::io::Error::last_os_error());
            }
        }
        #[cfg(windows)]
        lock.lock().with_context(|| format!("locking {}", lock_path.display()))?;
        let out = f();
        drop(lock);
        out
    }
}

pub fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::trees::{Owner, OwnerKind};

    pub(crate) fn lease(id: &str, path: &Path) -> Lease {
        Lease {
            id: id.into(),
            repo: "r".into(),
            repo_path: path.parent().unwrap().to_path_buf(),
            path: path.to_path_buf(),
            branch: Some("b".into()),
            base: "0".into(),
            owner: Owner { kind: OwnerKind::Human, id: "joe".into(), label: "joe".into() },
            state: TreeState::Active,
            created_at: "t".into(),
            updated_at: "t".into(),
            expires_at: None,
            seed: None,
            once: false,
            size_bytes: None,
            size_at: None,
            checkpoints: 0,
            last_checkpoint_at: None,
            last_checkpoint_tree: None,
        }
    }

    #[test]
    fn put_get_modify_find_remove() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(tree.join("src")).unwrap();
        let store = LeaseStore::at(dir.path().join("leases.json"));
        store.put(lease("r~b", &tree)).unwrap();
        assert!(store.get("r~b").unwrap().is_some());
        assert_eq!(store.modify("r~b", |l| { l.checkpoints += 1; l.checkpoints }).unwrap(), Some(1));
        assert_eq!(store.find_by_path(&tree.join("src")).unwrap().unwrap().id, "r~b");
        assert!(store.find_by_path(dir.path()).unwrap().is_none());
        assert!(store.remove("r~b").unwrap().is_some());
        assert!(store.list().unwrap().is_empty());
    }
}
