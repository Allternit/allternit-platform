//! The one canonical writer for personal memory. Every kernel path that used
//! to insert, retire or delete `memory_facts` rows (retain, model extraction,
//! fact edits, deletes, adapters, reconstruction) commits to the owner's
//! Memory Drive here instead; the rows are then rebuilt from git by
//! `memory_drive_service::reindex`.
//!
//! A database without a configured drive root (unit tests, offline tools)
//! returns `Ok(None)` so the caller keeps its legacy row path. A configured
//! database never falls back to rows: a failed commit is recorded in
//! `memory_drive_pending` and retried on the next write or read.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::DbHandle;
use crate::memory_drive::{DriveError, Entry, Operation};
use crate::memory_drive_service::{self as service, DriveRecord, Result, ServiceError};

const ROOT_KEY: &str = "brains_dir";
const MAX_CAS_RETRIES: usize = 6;
const MAX_PENDING_ATTEMPTS: i64 = 12;

/// Record the storage root for this database. Called once at startup with
/// the configured `brains_dir`, never derived from the environment here.
pub fn configure_root(db: &DbHandle, root: &Path) -> Result<()> {
    if !root.is_absolute() {
        return Err(ServiceError::Provenance("memory drive root must be absolute".into()));
    }
    db.connect()?.execute(
        "INSERT INTO memory_drive_config(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![ROOT_KEY, root.to_string_lossy()],
    )?;
    Ok(())
}

pub fn configured_root(conn: &Connection) -> Result<Option<PathBuf>> {
    Ok(conn
        .query_row("SELECT value FROM memory_drive_config WHERE key=?1", params![ROOT_KEY], |r| r.get::<_, String>(0))
        .optional()?
        .map(PathBuf::from))
}

/// The owner's drive, provisioned on first use. `None` when this database has
/// no drive root (the legacy row writers stay in charge).
pub fn ensure(db: &DbHandle, owner: &str) -> Result<Option<DriveRecord>> {
    let root = configured_root(&db.connect()?)?;
    let Some(root) = root else { return Ok(None) };
    match service::resolve(db, owner) {
        Ok(drive) => Ok(Some(drive)),
        Err(ServiceError::NotFound) => service::provision(db, &root, owner).map(Some),
        Err(e) => Err(e),
    }
}

/// One memory to add. Provenance is derived from the observation when the
/// caller has no explicit source.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewFact {
    pub text: String,
    pub memory_type: Option<String>,
    pub agent: Option<String>,
    pub observation: Option<String>,
    pub session: Option<String>,
    pub confidence: Option<f64>,
    /// Explicit `source:` value; validated like any other.
    pub source: Option<String>,
    /// Topic file; defaults by memory type.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PendingPayload {
    adds: Vec<NewFact>,
    retire: Vec<String>,
    message: String,
}

#[derive(Debug, Clone, Default)]
pub struct CommitOutcome {
    /// Index fact id per requested add, `None` when skipped (duplicate,
    /// possible secret, invalid text).
    pub fact_ids: Vec<Option<String>>,
    /// Retire ids that were legacy (non-drive) rows; the caller retires them
    /// in the archive the old way.
    pub legacy_retire: Vec<String>,
    pub revision: Option<String>,
}

pub fn topic_for(memory_type: Option<&str>) -> &'static str {
    match memory_type.unwrap_or("fact") {
        "preference" => "preferences.md",
        "event" => "events.md",
        "procedure" => "procedures.md",
        "entity" => "people-and-things.md",
        "relationship" => "relationships.md",
        "task_state" => "tasks.md",
        _ => "facts.md",
    }
}

pub fn new_entry_id() -> String {
    format!("m-{}", uuid::Uuid::new_v4().simple())
}

/// Session link for an observation's session, when the session is the
/// owner's, still exists and is not incognito.
fn session_source(conn: &Connection, owner: &str, fact: &NewFact) -> Result<(Option<String>, Option<String>)> {
    let session = match (&fact.session, &fact.observation) {
        (Some(s), _) => Some(s.clone()),
        (None, Some(obs)) => conn
            .query_row(
                "SELECT session_id FROM memory_observations WHERE id=?1 AND user_id=?2",
                params![obs, owner],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten(),
        _ => None,
    };
    if let Some(s) = session {
        if service::session_is_owned(conn, owner, &s)? {
            return Ok((Some(format!("/?session={s}")), Some(s)));
        }
    }
    Ok((None, None))
}

fn build_entry(conn: &Connection, owner: &str, fact: &NewFact, today: &str) -> Result<Option<(String, Entry)>> {
    let text = fact.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || crate::memory_kernel_service::mentions_secret(&text) {
        return Ok(None);
    }
    let (session_link, session) = session_source(conn, owner, fact)?;
    let source = fact.source.clone().or(session_link).unwrap_or_else(|| match &fact.observation {
        Some(obs) => format!("allternit:observation/{obs}"),
        None => "/?view=settings&section=memory".to_string(),
    });
    let mut entry = Entry { id: new_entry_id(), text, source, added: today.to_string(), metadata: Default::default() };
    if let Some(t) = &fact.memory_type {
        entry.metadata.insert("memory_type".into(), t.clone());
    }
    if let Some(obs) = &fact.observation {
        entry.metadata.insert("observation".into(), obs.clone());
    }
    if let Some(s) = session {
        entry.metadata.insert("session".into(), s);
    }
    if let Some(c) = fact.confidence.filter(|c| c.is_finite() && (0.0..=1.0).contains(c)) {
        entry.metadata.insert("confidence".into(), format!("{c:.2}"));
    }
    if let Some(agent) = &fact.agent {
        entry.metadata.insert("agent".into(), agent.clone());
        // An agent the account can't attest is dropped, not the memory.
        if service::check_provenance(conn, owner, &entry).is_err() {
            entry.metadata.remove("agent");
        }
    }
    if entry.render().is_err() || service::check_provenance(conn, owner, &entry).is_err() {
        return Ok(None);
    }
    let path = fact.path.clone().unwrap_or_else(|| topic_for(fact.memory_type.as_deref()).to_string());
    Ok(Some((path, entry)))
}

/// Add and retire memories in ONE drive commit (one per turn / edit), retrying
/// on concurrent writers. Returns `Ok(None)` when the database has no drive.
pub fn commit_facts(db: &DbHandle, owner: &str, adds: &[NewFact], retire: &[String], message: &str) -> Result<Option<CommitOutcome>> {
    let Some(drive) = ensure(db, owner)? else { return Ok(None) };
    // Earlier failed intents go first so ordering is preserved.
    retry_pending(db, owner);
    match commit_once(db, owner, &drive, adds, retire, message) {
        Ok(out) => Ok(Some(out)),
        Err(e) => {
            record_pending(db, owner, adds, retire, message, &e);
            Err(e)
        }
    }
}

fn commit_once(db: &DbHandle, owner: &str, drive: &DriveRecord, adds: &[NewFact], retire: &[String], message: &str) -> Result<CommitOutcome> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let conn = db.connect()?;
    let mut ops = Vec::new();
    let mut entry_ids: Vec<Option<String>> = Vec::with_capacity(adds.len());
    let mut seen = BTreeSet::new();
    for fact in adds {
        let lowered = fact.text.trim().to_lowercase();
        let duplicate = !seen.insert(lowered.clone())
            || conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_drive_entries e JOIN memory_facts f ON f.id=e.fact_id
                  WHERE e.drive_id=?1 AND lower(f.fact)=?2 AND f.valid_until IS NULL)",
                params![drive.id, lowered],
                |r| r.get::<_, bool>(0),
            )?;
        let retiring_same = duplicate && retire.iter().any(|id| {
            conn.query_row("SELECT lower(fact)=?2 FROM memory_facts WHERE id=?1", params![id, lowered], |r| r.get::<_, bool>(0))
                .unwrap_or(false)
        });
        if duplicate && !retiring_same {
            entry_ids.push(None);
            continue;
        }
        match build_entry(&conn, owner, fact, &today)? {
            Some((path, entry)) => {
                entry_ids.push(Some(entry.id.clone()));
                ops.push(Operation::UpsertEntry { path, entry });
            }
            None => entry_ids.push(None),
        }
    }
    let mut legacy_retire = Vec::new();
    for fact_id in retire {
        let entry: Option<String> = conn
            .query_row(
                "SELECT entry_id FROM memory_drive_entries WHERE drive_id=?1 AND fact_id=?2",
                params![drive.id, fact_id],
                |r| r.get(0),
            )
            .optional()?;
        match entry {
            Some(id) => ops.push(Operation::DeleteEntry { id }),
            None => legacy_retire.push(fact_id.clone()),
        }
    }
    drop(conn);
    let mut revision = None;
    if !ops.is_empty() {
        let mut attempt = 0;
        loop {
            let head = drive.storage()?.head()?.ok_or(DriveError::Uninitialized)?;
            match service::apply(db, owner, &head, &ops, message) {
                Ok(r) => {
                    revision = Some(r.revision);
                    break;
                }
                Err(ServiceError::Drive(DriveError::Conflict { .. })) if attempt < MAX_CAS_RETRIES => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(15 * attempt as u64));
                }
                Err(e) => return Err(e),
            }
        }
    }
    let conn = db.connect()?;
    let mut fact_ids = Vec::with_capacity(entry_ids.len());
    for id in entry_ids {
        fact_ids.push(match id {
            Some(id) => conn
                .query_row(
                    "SELECT fact_id FROM memory_drive_entries WHERE drive_id=?1 AND entry_id=?2",
                    params![drive.id, id],
                    |r| r.get(0),
                )
                .optional()?,
            None => None,
        });
    }
    Ok(CommitOutcome { fact_ids, legacy_retire, revision })
}

/// Rewrite one drive-backed fact's entry in place (same stable id). Returns
/// false when the fact is not drive-backed.
pub fn modify_fact_entry(db: &DbHandle, owner: &str, fact_id: &str, edit: impl Fn(&mut Entry)) -> Result<bool> {
    let Some(drive) = ensure(db, owner)? else { return Ok(false) };
    let mapped: Option<(String, String)> = db
        .connect()?
        .query_row(
            "SELECT file_path,entry_id FROM memory_drive_entries WHERE drive_id=?1 AND fact_id=?2",
            params![drive.id, fact_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((path, entry_id)) = mapped else { return Ok(false) };
    for attempt in 0..=MAX_CAS_RETRIES {
        let storage = drive.storage()?;
        let head = storage.head()?.ok_or(DriveError::Uninitialized)?;
        let content = storage.read_file(&path, Some(&head))?;
        let mut found = None;
        for line in content.lines().filter(|l| l.starts_with("- ") && !l.starts_with("- [[")) {
            let entry = Entry::parse(line)?;
            if entry.id == entry_id {
                found = Some(entry);
                break;
            }
        }
        let Some(mut entry) = found else { return Ok(false) };
        edit(&mut entry);
        match service::apply(db, owner, &head, &[Operation::UpsertEntry { path: path.clone(), entry }], "Edit memory") {
            Ok(_) => return Ok(true),
            Err(ServiceError::Drive(DriveError::Conflict { .. })) if attempt < MAX_CAS_RETRIES => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

pub fn is_drive_fact(conn: &Connection, fact_id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM memory_drive_entries WHERE fact_id=?1)", params![fact_id], |r| r.get(0))
}

fn record_pending(db: &DbHandle, owner: &str, adds: &[NewFact], retire: &[String], message: &str, error: &ServiceError) {
    // A commit that landed with indexing pending is not lost intent.
    if matches!(error, ServiceError::IndexPending { .. }) || (adds.is_empty() && retire.is_empty()) {
        return;
    }
    let payload = PendingPayload { adds: adds.to_vec(), retire: retire.to_vec(), message: message.to_string() };
    let Ok(payload) = serde_json::to_string(&payload) else { return };
    let saved = db.connect().and_then(|c| {
        c.execute(
            "INSERT INTO memory_drive_pending(id,user_id,payload,error,attempts) VALUES(?1,?2,?3,?4,1)",
            params![uuid::Uuid::new_v4().to_string(), owner, payload, error.to_string()],
        )
    });
    if let Err(e) = saved {
        tracing::error!("memory drive: could not record pending write: {e}");
    } else {
        tracing::warn!("memory drive write failed, kept as pending: {error}");
    }
}

/// Replays failed intents oldest first. Stops at the first that still fails.
pub fn retry_pending(db: &DbHandle, owner: &str) {
    let Ok(conn) = db.connect() else { return };
    let rows: Vec<(String, String)> = match conn
        .prepare("SELECT id,payload FROM memory_drive_pending WHERE user_id=?1 AND attempts<?2 ORDER BY created_at,rowid LIMIT 20")
        .and_then(|mut s| s.query_map(params![owner, MAX_PENDING_ATTEMPTS], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
    {
        Ok(rows) => rows,
        Err(_) => return,
    };
    let Ok(Some(drive)) = ensure(db, owner) else { return };
    for (id, payload) in rows {
        let Ok(p) = serde_json::from_str::<PendingPayload>(&payload) else {
            let _ = conn.execute("UPDATE memory_drive_pending SET attempts=?2,error='unreadable' WHERE id=?1", params![id, MAX_PENDING_ATTEMPTS]);
            continue;
        };
        match commit_once(db, owner, &drive, &p.adds, &p.retire, &p.message) {
            Ok(out) => {
                if !out.legacy_retire.is_empty() {
                    for f in &out.legacy_retire {
                        let _ = crate::memory_relations::supersede_fact(&conn, owner, f);
                    }
                }
                let _ = conn.execute("DELETE FROM memory_drive_pending WHERE id=?1", params![id]);
            }
            Err(e) => {
                let _ = conn.execute(
                    "UPDATE memory_drive_pending SET attempts=attempts+1,error=?2 WHERE id=?1",
                    params![id, e.to_string()],
                );
                break;
            }
        }
    }
}

pub fn pending_count(conn: &Connection, owner: &str) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM memory_drive_pending WHERE user_id=?1", params![owner], |r| r.get(0))
}
