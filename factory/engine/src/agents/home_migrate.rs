//! One-time move of the agent orchestrator's home, `~/.agent-orchestrator/`, // old-names: keep (the migration names the old home)
//! into the Factory home, `~/.allternit/factory/` (SPEC §12, on-disk state).
//!
//! The engine runs this at `serve` start; `allternit-factory internal
//! migrate-home [--dry-run] [--json]` runs it by hand, and `gizzi doctor`
//! reads its marker. Rules:
//! - Everything moves (logs, briefs, evidence, consults, the old registry
//!   `state.json`, `ORCHESTRATOR.md`, …). A name that's free in the Factory
//!   home is renamed into place; a folder that exists on both sides is merged
//!   child by child.
//! - Folders whose absolute paths are baked in stay where they are: a Python
//!   venv (`pyvenv.cfg`) or a linked checkout (a `.git` file). Moving them
//!   would break them; they're reported as `kept`.
//! - Nothing is overwritten or deleted. A file that exists on both sides is
//!   dropped from the old side only when the bytes are identical; otherwise it
//!   stays where it is and is reported as a conflict.
//! - The old folder is removed only once it's empty.
//! - The marker `migrated-agent-orchestrator.json` in the Factory home records
//!   what happened. With the marker present the move doesn't run again
//!   automatically, so a folder that reappears (an old script ran) is reported
//!   by `gizzi doctor` instead of silently swallowed.
//! - Only the default home migrates. With `$ALLTERNIT_FACTORY_HOME` set (tests,
//!   dev runs) the automatic move is skipped, so a test can never move a real
//!   home folder.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::registry::factory_home;

/// Marker file name under the Factory home.
pub const MARKER_FILE: &str = "migrated-agent-orchestrator.json";

/// The agent orchestrator's old home folder.
pub fn legacy_home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".agent-orchestrator") // old-names: keep (the folder this migrates from)
}

/// What a migration did (or, with `dry_run`, would do).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub from: String,
    pub to: String,
    pub at: String,
    pub dry_run: bool,
    /// Old-home paths (relative) moved into the Factory home.
    pub moved: Vec<String>,
    /// Identical files on both sides; the old copy was dropped.
    pub deduplicated: Vec<String>,
    /// Paths left in the old home because the Factory home has different
    /// contents under the same name.
    pub conflicts: Vec<String>,
    /// Paths left in place on purpose (venvs, linked checkouts: their
    /// absolute paths are baked in).
    #[serde(default)]
    pub kept: Vec<String>,
    /// The old folder was removed (it ended up empty).
    pub removed_old_home: bool,
}

/// Why the automatic move didn't run.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Migrated(MigrationReport),
    /// No `~/.agent-orchestrator/`. // old-names: keep (the old home)
    NothingToMigrate,
    /// The marker exists: it ran before.
    AlreadyMigrated,
    /// `$ALLTERNIT_FACTORY_HOME` is set.
    SkippedCustomHome,
}

/// The automatic move at engine start. Never fails the caller: errors come
/// back as `Err` for the caller to log, and the engine keeps starting.
pub fn migrate_default_home() -> Result<Outcome> {
    if std::env::var_os("ALLTERNIT_FACTORY_HOME").is_some_and(|d| !d.is_empty()) {
        return Ok(Outcome::SkippedCustomHome);
    }
    let to = factory_home();
    if to.join(MARKER_FILE).is_file() {
        return Ok(Outcome::AlreadyMigrated);
    }
    let from = legacy_home();
    if !from.is_dir() {
        return Ok(Outcome::NothingToMigrate);
    }
    migrate(&from, &to, false).map(Outcome::Migrated)
}

/// Move `from` into `to` (see the module rules). Writes the marker unless
/// `dry_run`.
pub fn migrate(from: &Path, to: &Path, dry_run: bool) -> Result<MigrationReport> {
    let mut report = MigrationReport {
        from: from.display().to_string(),
        to: to.display().to_string(),
        at: chrono::Utc::now().to_rfc3339(),
        dry_run,
        ..Default::default()
    };
    if from.is_dir() {
        if !dry_run {
            std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
        }
        move_tree(from, to, Path::new(""), dry_run, &mut report)?;
        if !dry_run && std::fs::remove_dir(from).is_ok() {
            report.removed_old_home = true;
        }
    }
    if !dry_run {
        let marker = to.join(MARKER_FILE);
        std::fs::write(&marker, serde_json::to_vec_pretty(&report)?)
            .with_context(|| format!("writing {}", marker.display()))?;
    }
    Ok(report)
}

/// Read the marker, if the move ran.
pub fn read_marker(home: &Path) -> Option<MigrationReport> {
    let bytes = std::fs::read(home.join(MARKER_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn move_tree(src: &Path, dst: &Path, rel: &Path, dry_run: bool, report: &mut MigrationReport) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(src)
        .with_context(|| format!("reading {}", src.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let s = entry.path();
        let d = dst.join(&name);
        let r = rel.join(&name);
        let r_s = r.display().to_string();
        let s_meta = std::fs::symlink_metadata(&s).with_context(|| format!("reading {}", s.display()))?;
        if s_meta.is_dir() && pinned(&s) {
            report.kept.push(r_s);
            continue;
        }
        match std::fs::symlink_metadata(&d) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !dry_run {
                    std::fs::rename(&s, &d)
                        .with_context(|| format!("moving {} to {}", s.display(), d.display()))?;
                }
                report.moved.push(r_s);
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", d.display())),
            Ok(d_meta) if s_meta.is_dir() && d_meta.is_dir() => {
                move_tree(&s, &d, &r, dry_run, report)?;
                if !dry_run {
                    // Empty once every child moved; a conflict keeps it.
                    let _ = std::fs::remove_dir(&s);
                }
            }
            Ok(d_meta) if s_meta.is_file() && d_meta.is_file() && same_bytes(&s, &d) => {
                if !dry_run {
                    std::fs::remove_file(&s).with_context(|| format!("removing {}", s.display()))?;
                }
                report.deduplicated.push(r_s);
            }
            Ok(_) => report.conflicts.push(r_s),
        }
    }
    Ok(())
}

/// A folder that can't move without breaking: a Python venv or a linked
/// checkout (its `.git` is a file pointing back at the main repository).
fn pinned(dir: &Path) -> bool {
    dir.join("pyvenv.cfg").is_file() || dir.join(".git").is_file()
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn moves_merges_and_keeps_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("old");
        let to = tmp.path().join("factory");
        write(&from.join("ORCHESTRATOR.md"), "doc");
        write(&from.join("state.json"), "{}");
        write(&from.join("logs/a.log"), "a");
        write(&from.join("logs/same.log"), "same");
        write(&from.join("logs/clash.log"), "old");
        write(&to.join("logs/same.log"), "same");
        write(&to.join("logs/clash.log"), "new");
        write(&to.join("registry.json"), "{}");
        write(&from.join("uhp-venv/pyvenv.cfg"), "home = /usr/bin");
        write(&from.join("uhp-venv/bin/python"), "");

        let dry = migrate(&from, &to, true).unwrap();
        assert!(dry.dry_run);
        assert!(from.join("logs/a.log").is_file(), "dry run moves nothing");
        assert!(!to.join(MARKER_FILE).exists(), "dry run writes no marker");

        let r = migrate(&from, &to, false).unwrap();
        assert_eq!(r.moved, vec!["ORCHESTRATOR.md", "logs/a.log", "state.json"]);
        assert_eq!(r.deduplicated, vec!["logs/same.log"]);
        assert_eq!(r.conflicts, vec!["logs/clash.log"]);
        assert_eq!(r.kept, vec!["uhp-venv"]);
        assert!(from.join("uhp-venv/bin/python").is_file(), "a venv stays where its paths point");
        assert!(!r.removed_old_home, "a conflict keeps the old folder");
        assert_eq!(std::fs::read_to_string(to.join("logs/clash.log")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(from.join("logs/clash.log")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(to.join("ORCHESTRATOR.md")).unwrap(), "doc");
        assert_eq!(read_marker(&to).unwrap().conflicts, vec!["logs/clash.log"]);
    }

    #[test]
    fn empty_old_home_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("old");
        let to = tmp.path().join("factory");
        write(&from.join("briefs/x.md"), "x");
        let r = migrate(&from, &to, false).unwrap();
        assert!(r.removed_old_home);
        assert!(!from.exists());
        assert!(to.join("briefs/x.md").is_file());
    }
}
