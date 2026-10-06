//! Bot and project memory on their drives. The bot's or project's Memory
//! Drive is the source of truth; `cowork_memory_entries` rows for that scope
//! are an index rebuilt from it (same model as personal memory and
//! `memory_facts`). The cowork runtime's readers, grants and search keep
//! working on the rows unchanged.
//!
//! Writers (store, forget, promote, weekly curation) commit to the drive
//! first, then reindex. Rows with no drive scope (unowned user-level entries,
//! cowork turn summaries owned by the workspace principal, bots that exist
//! only on this device) stay rows. Legacy rows of a scope are imported into
//! its drive once; rows that can't be (text that looks like a credential)
//! are left untouched as archive.
use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::DbHandle;
use crate::memory_drive::{DriveError, Entry, Operation};
use crate::memory_drive_scopes::{self as scopes, DriveRef};
use crate::memory_drive_service::{self as service, DriveRecord, Result, ServiceError};

const FILE: &str = "memory.md";
const MAX_CAS_RETRIES: usize = 6;

/// One cowork memory entry, as stored in a row.
#[derive(Debug, Clone, Default)]
pub struct CoworkEntry {
    pub id: String,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub content: String,
    pub type_: String,
    pub tags: Option<String>,
    pub source: Option<String>,
    pub owner_principal: Option<String>,
    pub grants: Vec<String>,
    pub created_at: Option<String>,
}

/// The drive a row belongs to: the bot's when a bot owns it, else the
/// project's. `None` keeps it a plain row.
pub fn scope_for(conn: &Connection, user: &str, owner_principal: Option<&str>, project_id: Option<&str>) -> rusqlite::Result<Option<DriveRef>> {
    if let Some(p) = owner_principal.filter(|p| !p.is_empty()) {
        let bot: Option<String> = conn
            .query_row(
                "SELECT id FROM agents WHERE user_id=?1 AND (principal_id=?2 OR 'a://local/bot/' || id = ?2) LIMIT 1",
                params![user, p],
                |r| r.get(0),
            )
            .optional()?;
        return Ok(bot.map(|b| DriveRef { kind: "bot".into(), scope_id: b }));
    }
    if let Some(project) = project_id.filter(|p| !p.is_empty()) {
        if scopes::access(conn, user, "project", project)?.is_some() {
            return Ok(Some(DriveRef { kind: "project".into(), scope_id: project.to_string() }));
        }
    }
    Ok(None)
}

fn safe_id(id: &str) -> Option<String> {
    (!id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then(|| id.to_string())
}

fn safe_value(v: &str) -> Option<String> {
    let v: String = v.split_whitespace().collect::<Vec<_>>().join(" ");
    let v = v.replace([';', '[', ']', '\\'], "");
    (!v.is_empty() && v.len() <= 512).then_some(v)
}

fn to_entry(e: &CoworkEntry) -> Option<Entry> {
    let id = safe_id(&e.id)?;
    let text = e.content.split_whitespace().collect::<Vec<_>>().join(" ");
    let source = match e.session_id.as_deref().and_then(safe_id) {
        Some(s) => format!("/?session={s}"),
        None => "allternit:cowork".to_string(),
    };
    let added = e
        .created_at
        .as_deref()
        .and_then(|d| d.get(..10))
        .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
        .map(str::to_string)
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let mut entry = Entry { id, text, source, added, metadata: Default::default() };
    let m = &mut entry.metadata;
    if let Some(t) = safe_value(&e.type_) {
        if crate::memory_relations::MemoryType::parse(&t).is_some() {
            m.insert("memory_type".into(), t.clone());
        }
        m.insert("cowork_type".into(), t);
    }
    if let Some(v) = e.tags.as_deref().and_then(safe_value) {
        m.insert("tags".into(), v);
    }
    if let Some(v) = e.source.as_deref().and_then(safe_value) {
        m.insert("cowork_source".into(), v);
    }
    if let Some(v) = e.owner_principal.as_deref().and_then(safe_value) {
        m.insert("owner".into(), v);
    }
    let grants: Vec<String> = e.grants.iter().filter_map(|g| safe_value(g)).filter(|g| !g.contains(',')).collect();
    if !grants.is_empty() {
        m.insert("grants".into(), grants.join(","));
    }
    if let Some(v) = e.project_id.as_deref().and_then(safe_value) {
        m.insert("project".into(), v);
    }
    if let Some(v) = e.session_id.as_deref().and_then(safe_id) {
        m.insert("session".into(), v);
    }
    if crate::memory_kernel_service::mentions_secret(&entry.text) || entry.render().is_err() {
        return None;
    }
    Some(entry)
}

fn scope_rows_sql(drive: &DriveRecord) -> &'static str {
    if drive.kind == "bot" {
        "SELECT id FROM cowork_memory_entries WHERE user_id=?1 AND owner_principal IN
           (SELECT principal_id FROM agents WHERE id=?2 AND principal_id IS NOT NULL UNION SELECT 'a://local/bot/' || ?2)"
    } else {
        "SELECT id FROM cowork_memory_entries WHERE user_id=?1 AND project_id=?2 AND (owner_principal IS NULL OR owner_principal='')"
    }
}

fn load_row(conn: &Connection, id: &str) -> rusqlite::Result<Option<CoworkEntry>> {
    conn.query_row(
        "SELECT id,project_id,session_id,content,type,tags,source,owner_principal,COALESCE(grants,'[]'),created_at
         FROM cowork_memory_entries WHERE id=?1",
        params![id],
        |r| {
            Ok(CoworkEntry {
                id: r.get(0)?,
                project_id: r.get(1)?,
                session_id: r.get(2)?,
                content: r.get(3)?,
                type_: r.get(4)?,
                tags: r.get(5)?,
                source: r.get(6)?,
                owner_principal: r.get(7)?,
                grants: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
                created_at: r.get::<_, Option<String>>(9)?,
            })
        },
    )
    .optional()
}

/// Commit `ops` to the drive (retrying concurrent writers), importing the
/// scope's legacy rows first, then rebuild the scope's rows.
fn commit(db: &DbHandle, user: &str, drive: &DriveRecord, ops: impl Fn(&BTreeSet<String>) -> Vec<Operation>, message: &str) -> Result<()> {
    ensure_imported(db, drive)?;
    for attempt in 0..=MAX_CAS_RETRIES {
        let storage = drive.storage()?;
        let head = storage.head()?.ok_or(DriveError::Uninitialized)?;
        let present: BTreeSet<String> = service::entries(&storage.snapshot(Some(&head))?)?.into_iter().map(|(_, e)| e.id).collect();
        let ops = ops(&present);
        if ops.is_empty() {
            break;
        }
        match scopes::apply_for(db, user, drive, &head, &ops, message) {
            Ok(_) => break,
            Err(ServiceError::Drive(DriveError::Conflict { .. })) if attempt < MAX_CAS_RETRIES => continue,
            Err(e) => return Err(e),
        }
    }
    reindex(db, drive)
}

fn open(db: &DbHandle, user: &str, scope: &DriveRef) -> Result<Option<DriveRecord>> {
    let conn = db.connect()?;
    let Some(root) = crate::memory_drive_writer::configured_root(&conn)? else { return Ok(None) };
    drop(conn);
    scopes::open(db, &root, user, scope, true).map(Some)
}

/// Store one entry. `Ok(None)` when it has no drive scope (or the database
/// has no drive root): the caller stores the row the old way.
pub fn store(db: &DbHandle, user: &str, mut entry: CoworkEntry) -> Result<Option<String>> {
    let scope = scope_for(&db.connect()?, user, entry.owner_principal.as_deref(), entry.project_id.as_deref())?;
    let Some(scope) = scope else { return Ok(None) };
    let Some(drive) = open(db, user, &scope)? else { return Ok(None) };
    entry.id = format!("mem_{}", uuid::Uuid::new_v4().simple());
    if entry.created_at.is_none() {
        entry.created_at = Some(chrono::Utc::now().to_rfc3339());
    }
    let e = to_entry(&entry).ok_or_else(|| ServiceError::Provenance("Nothing saved: it is empty, too long, or looks like a credential.".into()))?;
    commit(db, user, &drive, |_| vec![Operation::UpsertEntry { path: FILE.into(), entry: e.clone() }], "Remember")?;
    Ok(Some(entry.id))
}

/// Forget one entry. `Ok(None)` when the row isn't drive-backed.
pub fn forget(db: &DbHandle, user: &str, id: &str) -> Result<Option<bool>> {
    let conn = db.connect()?;
    let row: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT owner_principal, project_id, drive_id FROM cowork_memory_entries WHERE id=?1 AND user_id=?2",
            params![id, user],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((owner, project, drive_id)) = row else { return Ok(Some(false)) };
    if drive_id.is_none() {
        return Ok(None);
    }
    let Some(scope) = scope_for(&conn, user, owner.as_deref(), project.as_deref())? else { return Ok(None) };
    drop(conn);
    let Some(drive) = open(db, user, &scope)? else { return Ok(None) };
    let id = id.to_string();
    commit(db, user, &drive, |present| if present.contains(&id) { vec![Operation::DeleteEntry { id: id.clone() }] } else { vec![] }, "Forget memory")?;
    Ok(Some(true))
}

/// Move an entry to another scope (bot, project or none): removed from its
/// old drive, added to the new one, keeping its id and history line.
pub fn rescope(db: &DbHandle, user: &str, id: &str, owner_principal: Option<&str>, project_id: Option<&str>) -> Result<bool> {
    let conn = db.connect()?;
    let Some(mut entry) = load_row(&conn, id)? else { return Ok(false) };
    let from = scope_for(&conn, user, entry.owner_principal.as_deref(), entry.project_id.as_deref())?;
    let to = scope_for(&conn, user, owner_principal, project_id)?;
    drop(conn);
    entry.owner_principal = owner_principal.map(str::to_string);
    entry.project_id = project_id.map(str::to_string);
    entry.session_id = None;
    let eid = entry.id.clone();
    if let Some(drive) = from.as_ref().map(|s| open(db, user, s)).transpose()?.flatten() {
        commit(db, user, &drive, |present| if present.contains(&eid) { vec![Operation::DeleteEntry { id: eid.clone() }] } else { vec![] }, "Move memory out")?;
    }
    match to.as_ref().map(|s| open(db, user, s)).transpose()?.flatten() {
        Some(drive) => {
            if let Some(e) = to_entry(&entry) {
                commit(db, user, &drive, |_| vec![Operation::UpsertEntry { path: FILE.into(), entry: e.clone() }], "Move memory in")?;
                return Ok(true);
            }
            Ok(false)
        }
        None => {
            // No drive at the destination: it is a plain row again.
            let conn = db.connect()?;
            let n = conn.execute(
                "INSERT INTO cowork_memory_entries (id,user_id,project_id,session_id,content,type,tags,source,owner_principal,grants,created_at)
                 VALUES (?1,?2,?3,NULL,?4,?5,?6,?7,?8,?9,COALESCE(?10,CURRENT_TIMESTAMP))
                 ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id, session_id=NULL, owner_principal=excluded.owner_principal, drive_id=NULL",
                params![entry.id, user, entry.project_id, entry.content, entry.type_, entry.tags, entry.source, entry.owner_principal,
                    serde_json::to_string(&entry.grants).unwrap_or_else(|_| "[]".into()), entry.created_at],
            )?;
            Ok(n > 0)
        }
    }
}

/// Weekly curation of a bot's memory as one drive commit: merged entries
/// replace their sources, dropped entries are removed.
pub fn curate(db: &DbHandle, user: &str, principal: &str, merged: &[(String, Vec<String>)], dropped: &[String]) -> Result<bool> {
    let Some(scope) = scope_for(&db.connect()?, user, Some(principal), None)? else { return Ok(false) };
    let Some(drive) = open(db, user, &scope)? else { return Ok(false) };
    let today = chrono::Utc::now().to_rfc3339();
    let new: Vec<(Entry, Vec<String>)> = merged
        .iter()
        .filter_map(|(content, from)| {
            let e = CoworkEntry {
                id: format!("mem_{}", uuid::Uuid::new_v4().simple()),
                content: content.clone(),
                type_: "fact".into(),
                source: Some("curation".into()),
                owner_principal: Some(principal.to_string()),
                created_at: Some(today.clone()),
                ..Default::default()
            };
            to_entry(&e).map(|e| (e, from.clone()))
        })
        .collect();
    commit(
        db,
        user,
        &drive,
        |present| {
            let mut ops = Vec::new();
            for (e, from) in &new {
                ops.push(Operation::UpsertEntry { path: FILE.into(), entry: e.clone() });
                ops.extend(from.iter().filter(|id| present.contains(*id)).map(|id| Operation::DeleteEntry { id: id.clone() }));
            }
            ops.extend(dropped.iter().filter(|id| present.contains(*id)).map(|id| Operation::DeleteEntry { id: id.clone() }));
            ops
        },
        "Tidy bot memory",
    )?;
    Ok(true)
}

/// Import the scope's legacy rows into the drive once (one commit).
pub fn ensure_imported(db: &DbHandle, drive: &DriveRecord) -> Result<()> {
    if !matches!(drive.kind.as_str(), "bot" | "project") || drive.imported_at.is_some() {
        return Ok(());
    }
    let conn = db.connect()?;
    let fresh: bool = conn.query_row("SELECT imported_at IS NULL FROM memory_drives WHERE id=?1", params![drive.id], |r| r.get(0))?;
    if !fresh {
        return Ok(());
    }
    let ids: Vec<String> = {
        let mut stmt = conn.prepare(scope_rows_sql(drive))?;
        let rows = stmt.query_map(params![drive.user_id, drive.scope_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        rows
    };
    let mut entries = Vec::new();
    for id in ids {
        if let Some(e) = load_row(&conn, &id)?.and_then(|r| to_entry(&r)) {
            entries.push(e);
        }
    }
    drop(conn);
    for attempt in 0..=MAX_CAS_RETRIES {
        let storage = drive.storage()?;
        let head = storage.head()?.ok_or(DriveError::Uninitialized)?;
        let present: BTreeSet<String> = service::entries(&storage.snapshot(Some(&head))?)?.into_iter().map(|(_, e)| e.id).collect();
        let ops: Vec<Operation> = entries
            .iter()
            .filter(|e| !present.contains(&e.id))
            .map(|e| Operation::UpsertEntry { path: FILE.into(), entry: e.clone() })
            .collect();
        if ops.is_empty() {
            break;
        }
        match storage.apply_batch(Some(&head), &ops, "Import existing memory", "Allternit") {
            Ok(_) => break,
            Err(DriveError::Conflict { .. }) if attempt < MAX_CAS_RETRIES => continue,
            Err(e) => return Err(e.into()),
        }
    }
    db.connect()?.execute("UPDATE memory_drives SET imported_at=CURRENT_TIMESTAMP WHERE id=?1 AND imported_at IS NULL", params![drive.id])?;
    reindex(db, drive)
}

/// Rebuild the scope's rows from the drive. Only rows that came from this
/// drive are removed when their entry is gone; archive rows are untouched.
pub fn reindex(db: &DbHandle, drive: &DriveRecord) -> Result<()> {
    if !matches!(drive.kind.as_str(), "bot" | "project") {
        return Ok(());
    }
    let snapshot = drive.storage()?.snapshot(None)?;
    let mut conn = db.connect()?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let bot_principal = if drive.kind == "bot" {
        crate::cowork_routes::bot_memory_principal(&tx, &drive.user_id, &drive.scope_id)?
    } else {
        None
    };
    let mut seen = BTreeSet::new();
    for (path, e) in service::entries(&snapshot)? {
        if path.starts_with("twin/") || path == scopes::QUESTIONS || matches!(e.metadata.get("kind").map(String::as_str), Some("question" | "answer")) {
            continue;
        }
        seen.insert(e.id.clone());
        let m = &e.metadata;
        let project = m.get("project").cloned().or_else(|| (drive.kind == "project").then(|| drive.scope_id.clone()));
        let owner = if drive.kind == "bot" { m.get("owner").cloned().or_else(|| bot_principal.clone()) } else { None };
        let grants: Vec<&str> = m.get("grants").map(|g| g.split(',').collect()).unwrap_or_default();
        let ty = m.get("cowork_type").or_else(|| m.get("memory_type")).cloned().unwrap_or_else(|| "fact".into());
        tx.execute(
            "INSERT INTO cowork_memory_entries (id,user_id,project_id,session_id,content,type,tags,source,owner_principal,grants,created_at,drive_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id, session_id=excluded.session_id, content=excluded.content,
               type=excluded.type, tags=excluded.tags, source=excluded.source, owner_principal=excluded.owner_principal,
               grants=excluded.grants, drive_id=excluded.drive_id",
            params![e.id, drive.user_id, project, m.get("session"), e.text, ty, m.get("tags"),
                m.get("cowork_source").cloned().unwrap_or_else(|| e.source.clone()), owner,
                serde_json::to_string(&grants).unwrap_or_else(|_| "[]".into()), e.added, drive.id],
        )?;
    }
    let stale: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM cowork_memory_entries WHERE drive_id=?1")?;
        let rows = stmt.query_map(params![drive.id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for id in stale.into_iter().filter(|id| !seen.contains(id)) {
        tx.execute("DELETE FROM cowork_memory_entries WHERE id=?1 AND drive_id=?2", params![id, drive.id])?;
    }
    tx.commit()?;
    Ok(())
}
