//! The one session registry: `~/.allternit/factory/registry.json`.
//!
//! Every agent pane the engine starts (`agents up`, `workflows drive`) is
//! recorded here, keyed by its session label (`ao-<slug>`). The record is a
//! claim, never the truth: [`Registry::reconcile`] checks it against the live
//! panes at engine start and on every `agents ps`, so a session whose pane is
//! gone reads `dead`, never `running` (SPEC §6 rule 3).
//!
//! Migrated once from the agent orchestrator's old registry `state.json`
//! (same entry shape; the factory adds fields). The old file is left in place;
//! [`super::home_migrate`] later moves it into the Factory home with the rest
//! of the old folder.
//!
//! Every pane maps to a bot. A spawn with no bot gets a local placeholder
//! binding (`local:<slug>`, `placeholder: true`) that allternit-api can later
//! bind to a real Bot.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::backend::LivePane;

/// File name under [`factory_home`].
pub const REGISTRY_FILE: &str = "registry.json";

/// `$ALLTERNIT_FACTORY_HOME`, else `~/.allternit/factory`.
pub fn factory_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("ALLTERNIT_FACTORY_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    home_dir().join(".allternit/factory")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Where agent transcripts, runner files and spawn-gate settings live.
pub fn logs_dir() -> PathBuf {
    factory_home().join("logs")
}

/// The agent orchestrator's registry, migrated from once: in the old home,
/// or in the Factory home once [`super::home_migrate`] moved it there.
pub fn legacy_state_path() -> PathBuf {
    let old = super::home_migrate::legacy_home().join("state.json");
    if old.is_file() {
        return old;
    }
    factory_home().join("state.json")
}

/// The session label for a slug.
pub fn session_of(slug: &str) -> String {
    format!("ao-{slug}")
}

/// The slug of a session label (`ao-<slug>` → `<slug>`).
pub fn slug_of(session: &str) -> &str {
    session.strip_prefix("ao-").unwrap_or(session)
}

/// The bot a pane belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRef {
    /// Bot id (`local:<slug>` for a placeholder).
    pub id: String,
    /// True until allternit-api binds the pane to a real Bot.
    #[serde(default)]
    pub placeholder: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl BotRef {
    pub fn placeholder(slug: &str) -> Self {
        Self { id: format!("local:{slug}"), placeholder: true, ..Default::default() }
    }
}

/// One session. The first fields are the agent orchestrator's (kept so its
/// records load unchanged); the rest are the factory's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub dead: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// running | dead | finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,
    /// engine | tmux (tmux only on records from before the pane engine).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    #[serde(default)]
    pub queued: u32,
    // ---- factory additions ----
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    /// The harness the pane runs (`claude`, `codex`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<BotRef>,
    /// The harness argv as asked for, before the spawn gate rewrote it
    /// (`agents recover` rebuilds the launch from it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Extra pane environment the spawn asked for (a team bot's identity:
    /// `ALLTERNIT_FACTORY_BOT`, `…_TEAM`, …). Never secrets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The WIH the session was bound to (its Gate 2 policy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wih: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl Entry {
    pub fn is_running(&self) -> bool {
        !self.dead && self.lifecycle.as_deref().map_or(true, |l| l == "running")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryFile {
    #[serde(default)]
    pub sessions: BTreeMap<String, Entry>,
    /// Set once, when the file was created from the agent orchestrator's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<String>,
}

/// What [`Registry::reconcile`] changed, one line per session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub session: String,
    /// `dead` (record said running, pane is gone), `alive` (record said
    /// dead, pane is live), `adopted` (a live pane had no record),
    /// `bound` (a record got its placeholder bot), `pane` (pane id moved).
    pub kind: String,
}

/// The registry file plus its lock.
#[derive(Debug, Clone)]
pub struct Registry {
    path: PathBuf,
}

impl Default for Registry {
    fn default() -> Self {
        Self::open_default()
    }
}

impl Registry {
    /// `~/.allternit/factory/registry.json` (see [`factory_home`]).
    pub fn open_default() -> Self {
        Self { path: factory_home().join(REGISTRY_FILE) }
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the registry, migrating the agent orchestrator's once.
    pub fn load(&self) -> Result<RegistryFile> {
        self.with_lock(|| self.read_or_migrate())
    }

    /// Read-modify-write under the registry lock.
    pub fn update<T>(&self, f: impl FnOnce(&mut RegistryFile) -> T) -> Result<T> {
        self.with_lock(|| {
            let mut file = self.read_or_migrate()?;
            let out = f(&mut file);
            self.write(&file)?;
            Ok(out)
        })
    }

    /// Insert or replace one session.
    pub fn upsert(&self, session: &str, entry: Entry) -> Result<()> {
        self.update(|file| {
            file.sessions.insert(session.to_string(), entry);
        })
    }

    /// Check every record against the live panes and fix the record:
    /// gone panes become dead, live panes without a record are adopted, and
    /// every record gets a bot. Returns what changed (empty when nothing did).
    pub fn reconcile(&self, live: &[LivePane]) -> Result<Vec<Change>> {
        self.update(|file| reconcile_file(file, live, &now()))
    }

    fn read_or_migrate(&self) -> Result<RegistryFile> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("registry {} is not valid JSON", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let legacy = legacy_state_path();
                // Only the default registry migrates (tests and `at` paths don't).
                if self.path != factory_home().join(REGISTRY_FILE) || !legacy.is_file() {
                    return Ok(RegistryFile::default());
                }
                let bytes = std::fs::read(&legacy)
                    .with_context(|| format!("reading {}", legacy.display()))?;
                let mut file: RegistryFile = serde_json::from_slice(&bytes).unwrap_or_default();
                file.migrated_from = Some(legacy.display().to_string());
                self.write(&file)?;
                Ok(file)
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn write(&self, file: &RegistryFile) -> Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(file)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
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
        let out = f();
        drop(lock); // closing the descriptor releases the flock
        out
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// [`Registry::reconcile`] on an in-memory file (pure; `now` is passed in so
/// replaying the same inputs gives the same output).
pub fn reconcile_file(file: &mut RegistryFile, live: &[LivePane], now: &str) -> Vec<Change> {
    let mut changes = Vec::new();
    let live_by_session: BTreeMap<&str, &LivePane> =
        live.iter().map(|p| (p.session.as_str(), p)).collect();
    for (session, entry) in file.sessions.iter_mut() {
        let mut changed = false;
        match live_by_session.get(session.as_str()) {
            None if !entry.dead => {
                entry.dead = true;
                if entry.lifecycle.as_deref() != Some("finished") {
                    entry.lifecycle = Some("dead".to_string());
                }
                changes.push(Change { session: session.clone(), kind: "dead".into() });
                changed = true;
            }
            Some(pane) => {
                if entry.dead {
                    entry.dead = false;
                    entry.lifecycle = Some("running".to_string());
                    changes.push(Change { session: session.clone(), kind: "alive".into() });
                    changed = true;
                }
                if entry.pane_id.as_deref() != Some(pane.pane_id.as_str()) {
                    entry.pane_id = Some(pane.pane_id.clone());
                    changes.push(Change { session: session.clone(), kind: "pane".into() });
                    changed = true;
                }
            }
            None => {}
        }
        if entry.bot.is_none() {
            entry.bot = Some(BotRef::placeholder(slug_of(session)));
            changes.push(Change { session: session.clone(), kind: "bound".into() });
            changed = true;
        }
        if changed {
            entry.updated_at = Some(now.to_string());
        }
    }
    for pane in live {
        if file.sessions.contains_key(&pane.session) {
            continue;
        }
        file.sessions.insert(
            pane.session.clone(),
            Entry {
                cwd: pane.cwd.clone().unwrap_or_default(),
                lifecycle: Some("running".to_string()),
                world: Some("engine".to_string()),
                pane_id: Some(pane.pane_id.clone()),
                bot: Some(BotRef::placeholder(slug_of(&pane.session))),
                created_at: Some(now.to_string()),
                updated_at: Some(now.to_string()),
                ..Default::default()
            },
        );
        changes.push(Change { session: pane.session.clone(), kind: "adopted".into() });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(session: &str, id: &str) -> LivePane {
        LivePane { session: session.into(), pane_id: id.into(), cwd: Some("/w".into()), agent_status: None }
    }

    #[test]
    fn gone_pane_is_dead_and_live_pane_is_adopted() {
        let mut file = RegistryFile::default();
        file.sessions.insert(
            "ao-a".into(),
            Entry { lifecycle: Some("running".into()), ..Default::default() },
        );
        let changes = reconcile_file(&mut file, &[pane("ao-b", "p2")], "t");
        let a = &file.sessions["ao-a"];
        assert!(a.dead);
        assert_eq!(a.lifecycle.as_deref(), Some("dead"));
        assert_eq!(a.bot.as_ref().unwrap().id, "local:a");
        let b = &file.sessions["ao-b"];
        assert!(!b.dead && b.is_running());
        assert_eq!(b.pane_id.as_deref(), Some("p2"));
        assert!(b.bot.as_ref().unwrap().placeholder);
        let kinds: Vec<_> = changes.iter().map(|c| (c.session.as_str(), c.kind.as_str())).collect();
        assert_eq!(kinds, vec![("ao-a", "dead"), ("ao-a", "bound"), ("ao-b", "adopted")]);
        // A second pass with the same reality changes nothing.
        assert!(reconcile_file(&mut file, &[pane("ao-b", "p2")], "t2").is_empty());
    }

    #[test]
    fn finished_stays_finished_when_its_pane_goes() {
        let mut file = RegistryFile::default();
        file.sessions.insert(
            "ao-a".into(),
            Entry { lifecycle: Some("finished".into()), bot: Some(BotRef::placeholder("a")), ..Default::default() },
        );
        reconcile_file(&mut file, &[], "t");
        assert!(file.sessions["ao-a"].dead);
        assert_eq!(file.sessions["ao-a"].lifecycle.as_deref(), Some("finished"));
    }

    #[test]
    fn legacy_state_shape_loads() {
        let legacy = r#"{"sessions":{"ao-x":{"cwd":"/r","log":"/l","dead":false,"runner":"/c.sh","lead":"me","lifecycle":"running","world":"engine","queued":2}}}"#;
        let file: RegistryFile = serde_json::from_str(legacy).unwrap();
        let x = &file.sessions["ao-x"];
        assert_eq!(x.queued, 2);
        assert_eq!(x.lead.as_deref(), Some("me"));
        assert!(x.bot.is_none());
    }

    #[test]
    fn update_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::at(dir.path().join("registry.json"));
        reg.upsert("ao-z", Entry { cwd: "/z".into(), ..Default::default() }).unwrap();
        assert_eq!(reg.load().unwrap().sessions["ao-z"].cwd, "/z");
    }
}
