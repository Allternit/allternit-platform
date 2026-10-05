//! Spawn caps shared by every `drive` process on the machine (audit S10), and
//! capacity admission.
//!
//! State lives in `.allternit/drive/caps.json` and is only read-modified-written
//! under an exclusive `flock` on `.allternit/drive/caps.lock`, so two drive
//! processes (different DAGs) see one set of running sessions and one spawn
//! history. A running entry is dropped once its tmux session is gone; a
//! reservation younger than [`RESERVATION_GRACE_SECS`] is kept even before
//! its session exists, so a concurrent prune cannot race a spawn in flight.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};

use super::config::drive_dir;

pub const RESERVATION_GRACE_SECS: i64 = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapsState {
    #[serde(default)]
    pub running: Vec<RunningEntry>,
    #[serde(default)]
    pub spawns: Vec<SpawnEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningEntry {
    pub dag_id: String,
    pub node_id: String,
    /// Orchestrator slug (session `ao-<slug>`).
    pub slug: String,
    pub reserved_at: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnEntry {
    pub dag_id: String,
    pub node_id: String,
    pub at: String,
}

#[derive(Debug, Clone, Copy)]
pub struct CapLimits {
    pub dag_concurrent: usize,
    pub dag_per_hour: usize,
    pub global_concurrent: usize,
    pub global_per_hour: usize,
}

/// Why a spawn was deferred. `kind` is stable (ledger `DriveSpawnDeferred.reason`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deferral {
    pub kind: &'static str,
    pub limit: usize,
    pub current: usize,
}

impl std::fmt::Display for Deferral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}/{})", self.kind, self.current, self.limit)
    }
}

/// Snapshot counts for a DAG (for status lines and dry-run).
#[derive(Debug, Clone, Copy, Default)]
pub struct CapCounts {
    pub dag_running: usize,
    pub dag_last_hour: usize,
    pub global_running: usize,
    pub global_last_hour: usize,
}

pub struct CapsStore {
    dir: PathBuf,
}

impl CapsStore {
    pub fn new(root: &Path) -> Self {
        Self { dir: drive_dir(root) }
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("caps.json")
    }

    /// Run `f` on the state under the exclusive lock, pruning first, and
    /// persist the result.
    fn with_lock<T>(
        &self,
        alive: &dyn Fn(&str) -> bool,
        f: impl FnOnce(&mut CapsState) -> T,
    ) -> Result<T> {
        ensure_dir(&self.dir)?;
        let _lock = FileLock::exclusive(&self.dir.join("caps.lock"))?;
        let mut state: CapsState = read_json(&self.state_path())?.unwrap_or_default();
        prune(&mut state, Utc::now(), alive);
        let out = f(&mut state);
        write_json_atomic(&self.state_path(), &state)?;
        Ok(out)
    }

    /// Reserve a slot for one spawn, or say which cap defers it. A reservation
    /// counts as running and as a spawn in the hourly window.
    pub fn try_reserve(
        &self,
        entry: RunningEntry,
        limits: CapLimits,
        alive: &dyn Fn(&str) -> bool,
    ) -> Result<std::result::Result<(), Deferral>> {
        self.with_lock(alive, |state| {
            let counts = counts(state, &entry.dag_id, Utc::now());
            let checks = [
                ("global_max_concurrent", limits.global_concurrent, counts.global_running),
                ("max_concurrent", limits.dag_concurrent, counts.dag_running),
                ("global_max_spawns_per_hour", limits.global_per_hour, counts.global_last_hour),
                ("max_spawns_per_hour", limits.dag_per_hour, counts.dag_last_hour),
            ];
            for (kind, limit, current) in checks {
                if current >= limit {
                    return Err(Deferral { kind, limit, current });
                }
            }
            state.spawns.push(SpawnEntry {
                dag_id: entry.dag_id.clone(),
                node_id: entry.node_id.clone(),
                at: entry.reserved_at.clone(),
            });
            state.running.retain(|r| r.slug != entry.slug);
            state.running.push(entry);
            Ok(())
        })
    }

    /// Record an adopted session as running without counting a new spawn.
    pub fn adopt(&self, entry: RunningEntry, alive: &dyn Fn(&str) -> bool) -> Result<()> {
        self.with_lock(alive, |state| {
            if !state.running.iter().any(|r| r.slug == entry.slug) {
                state.running.push(entry);
            }
        })
    }

    /// Free a running slot (the spawn stays in the hourly window).
    pub fn release(&self, slug: &str, alive: &dyn Fn(&str) -> bool) -> Result<()> {
        self.with_lock(alive, |state| state.running.retain(|r| r.slug != slug))
    }

    /// Read-only counts (no lock, no prune of dead sessions, no write) — for
    /// dry-run and status output.
    pub fn peek(&self, dag_id: &str) -> Result<CapCounts> {
        let state: CapsState = read_json(&self.state_path())?.unwrap_or_default();
        Ok(counts(&state, dag_id, Utc::now()))
    }
}

fn parse_ts(ts: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts).ok().map(|d| d.with_timezone(&Utc))
}

fn prune(state: &mut CapsState, now: DateTime<Utc>, alive: &dyn Fn(&str) -> bool) {
    let hour_ago = now - Duration::hours(1);
    state
        .spawns
        .retain(|s| parse_ts(&s.at).is_some_and(|t| t > hour_ago));
    let grace = now - Duration::seconds(RESERVATION_GRACE_SECS);
    state.running.retain(|r| {
        parse_ts(&r.reserved_at).is_some_and(|t| t > grace) || alive(&r.slug)
    });
}

fn counts(state: &CapsState, dag_id: &str, now: DateTime<Utc>) -> CapCounts {
    let hour_ago = now - Duration::hours(1);
    let recent: Vec<&SpawnEntry> = state
        .spawns
        .iter()
        .filter(|s| parse_ts(&s.at).is_some_and(|t| t > hour_ago))
        .collect();
    CapCounts {
        dag_running: state.running.iter().filter(|r| r.dag_id == dag_id).count(),
        dag_last_hour: recent.iter().filter(|s| s.dag_id == dag_id).count(),
        global_running: state.running.len(),
        global_last_hour: recent.len(),
    }
}

/// Exclusive advisory lock held for the guard's lifetime.
pub struct FileLock {
    _file: File,
}

impl FileLock {
    /// Block until the lock is held.
    pub fn exclusive(path: &Path) -> Result<Self> {
        let file = open_lock_file(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: flock on a valid, owned fd.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc != 0 {
                return Err(std::io::Error::last_os_error())
                    .with_context(|| format!("flock {}", path.display()));
            }
        }
        Ok(Self { _file: file })
    }

    /// `Ok(None)` when another process holds it.
    pub fn try_exclusive(path: &Path) -> Result<Option<Self>> {
        let file = open_lock_file(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: flock on a valid, owned fd.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    return Ok(None);
                }
                return Err(err).with_context(|| format!("flock {}", path.display()));
            }
        }
        Ok(Some(Self { _file: file }))
    }
}

impl FileLock {
    /// [`FileLock::try_exclusive`], retried for up to `patience`.
    pub async fn try_exclusive_for(path: &Path, patience: std::time::Duration) -> Result<Option<Self>> {
        let deadline = std::time::Instant::now() + patience;
        loop {
            if let Some(lock) = Self::try_exclusive(path)? {
                return Ok(Some(lock));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

fn open_lock_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("opening lock {}", path.display()))
}

/// Machine capacity reading for admission.
#[derive(Debug, Clone, Copy)]
pub struct Capacity {
    pub load_per_cpu: Option<f64>,
    pub free_mem_mb: Option<u64>,
}

impl Capacity {
    pub fn probe() -> Self {
        Self {
            load_per_cpu: load_per_cpu(),
            free_mem_mb: free_mem_mb(),
        }
    }

    /// `Err(reason)` when below thresholds. An unreadable metric is not a refusal.
    pub fn admit(&self, min_free_mem_mb: u64, max_load_per_cpu: f64) -> std::result::Result<(), String> {
        if let Some(load) = self.load_per_cpu {
            if load > max_load_per_cpu {
                return Err(format!(
                    "load average per CPU {load:.2} is above max_load_per_cpu {max_load_per_cpu:.2}"
                ));
            }
        }
        if let Some(free) = self.free_mem_mb {
            if free < min_free_mem_mb {
                return Err(format!(
                    "available memory {free} MiB is below min_free_mem_mb {min_free_mem_mb}"
                ));
            }
        }
        Ok(())
    }
}

fn load_per_cpu() -> Option<f64> {
    #[cfg(unix)]
    {
        let mut loads = [0f64; 3];
        // SAFETY: getloadavg writes at most 3 doubles into the buffer.
        let n = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
        if n < 1 {
            return None;
        }
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        Some(loads[0] / cpus)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Available memory in MiB: `MemAvailable` on Linux; on macOS free +
/// inactive + speculative + purgeable pages from `vm_stat` (the pages the
/// kernel hands out without swapping).
fn free_mem_mb() -> Option<u64> {
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        return text
            .lines()
            .find(|l| l.starts_with("MemAvailable:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb / 1024);
    }
    let out = std::process::Command::new("vm_stat").output().ok()?;
    parse_vm_stat(&String::from_utf8_lossy(&out.stdout))
}

fn parse_vm_stat(text: &str) -> Option<u64> {
    let page_size: u64 = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let mut pages = 0u64;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        if matches!(
            key.trim(),
            "Pages free" | "Pages inactive" | "Pages speculative" | "Pages purgeable"
        ) {
            pages += value.trim().trim_end_matches('.').parse::<u64>().unwrap_or(0);
        }
    }
    Some(pages * page_size / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dag: &str, slug: &str) -> RunningEntry {
        RunningEntry {
            dag_id: dag.into(),
            node_id: slug.into(),
            slug: slug.into(),
            reserved_at: Utc::now().to_rfc3339(),
            pid: 1,
        }
    }

    #[test]
    fn caps_defer_by_dag_then_global_and_release_frees() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CapsStore::new(tmp.path());
        let limits = CapLimits { dag_concurrent: 1, dag_per_hour: 10, global_concurrent: 2, global_per_hour: 10 };
        let alive = |_: &str| true;
        assert!(store.try_reserve(entry("a", "a1"), limits, &alive).unwrap().is_ok());
        let d = store.try_reserve(entry("a", "a2"), limits, &alive).unwrap().unwrap_err();
        assert_eq!(d.kind, "max_concurrent");
        assert!(store.try_reserve(entry("b", "b1"), limits, &alive).unwrap().is_ok());
        let d = store.try_reserve(entry("c", "c1"), limits, &alive).unwrap().unwrap_err();
        assert_eq!(d.kind, "global_max_concurrent");
        store.release("a1", &alive).unwrap();
        assert!(store.try_reserve(entry("c", "c1"), limits, &alive).unwrap().is_ok());
        assert_eq!(store.peek("a").unwrap().dag_last_hour, 1);
        assert_eq!(store.peek("x").unwrap().global_last_hour, 3);
    }

    #[test]
    fn hourly_cap_counts_released_spawns() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CapsStore::new(tmp.path());
        let limits = CapLimits { dag_concurrent: 5, dag_per_hour: 2, global_concurrent: 5, global_per_hour: 5 };
        let alive = |_: &str| true;
        for s in ["1", "2"] {
            store.try_reserve(entry("a", s), limits, &alive).unwrap().unwrap();
            store.release(s, &alive).unwrap();
        }
        let d = store.try_reserve(entry("a", "3"), limits, &alive).unwrap().unwrap_err();
        assert_eq!(d.kind, "max_spawns_per_hour");
    }

    #[test]
    fn dead_sessions_past_grace_are_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CapsStore::new(tmp.path());
        let limits = CapLimits { dag_concurrent: 1, dag_per_hour: 10, global_concurrent: 1, global_per_hour: 10 };
        let mut old = entry("a", "old");
        old.reserved_at = (Utc::now() - Duration::seconds(RESERVATION_GRACE_SECS + 5)).to_rfc3339();
        store.try_reserve(old, limits, &|_| true).unwrap().unwrap();
        // Session gone: the next reservation prunes it.
        assert!(store.try_reserve(entry("a", "new"), limits, &|_| false).unwrap().is_ok());
    }

    #[test]
    fn vm_stat_parse_and_capacity_admission() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               1000.\nPages active:                            5.\nPages inactive:                           1000.\nPages speculative:                         48.\n";
        assert_eq!(parse_vm_stat(text), Some(2048 * 16384 / (1024 * 1024)));
        let c = Capacity { load_per_cpu: Some(3.0), free_mem_mb: Some(100) };
        assert!(c.admit(50, 4.0).is_ok());
        assert!(c.admit(200, 4.0).unwrap_err().contains("memory"));
        assert!(c.admit(50, 1.0).unwrap_err().contains("load"));
        assert!(Capacity { load_per_cpu: None, free_mem_mb: None }.admit(u64::MAX, 0.0).is_ok());
    }
}
