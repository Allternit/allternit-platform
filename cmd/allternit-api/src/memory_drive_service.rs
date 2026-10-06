//! Hosted personal drive registry, recoverable SQLite projection and bounded
//! kernel import. Callers provide the authenticated owner and explicit root.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::DbHandle;
use crate::memory_drive::{self as core, ApplyResult, DriveError, Entry, MemoryDrive, Operation, Snapshot};
use crate::memory_index;

pub type Result<T> = std::result::Result<T, ServiceError>;
const MAX_IMPORT_ROWS: usize = 800;
const RECEIPT: &str = "imports/kernel.md";

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Drive(#[from] DriveError),
    #[error("memory index storage unavailable")]
    Database(#[from] rusqlite::Error),
    #[error("memory drive not found for this account; open drive info first")]
    NotFound,
    #[error("you don't have access to this memory drive")]
    Forbidden,
    #[error("owner directory collides with another registered account")]
    OwnerCollision,
    #[error("invalid memory provenance: {0}")]
    Provenance(String),
    #[error("git commit {revision} saved; indexing is pending; retry a read or reindex")]
    IndexPending { revision: String },
    #[error("import exceeds the bounded plan; reduce the legacy set before importing")]
    ImportLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveRecord {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub brain_id: String,
    pub repo_path: PathBuf,
    pub branch: String,
    pub indexed_revision: Option<String>,
    pub dirty_revision: Option<String>,
    pub imported_at: Option<String>,
    pub import_revision: Option<String>,
    pub kind: String,
    pub scope_id: String,
}
impl DriveRecord {
    pub fn storage(&self) -> Result<MemoryDrive> {
        Ok(MemoryDrive::new(self.repo_path.clone(), &self.user_id, &self.branch)?)
    }
}

pub fn directory_key(owner: &str) -> Result<String> {
    if owner.is_empty() || owner.len() > 256 || owner.chars().any(char::is_control) {
        return Err(ServiceError::Provenance("invalid authenticated owner".into()));
    }
    Ok(owner.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect())
}

const RECORD_SQL: &str = "SELECT d.id,d.user_id,d.name,d.brain_id,d.repo_path,d.branch,d.indexed_revision,d.dirty_revision,d.imported_at,d.import_revision,d.kind,d.scope_id
     FROM memory_drives d JOIN brains b ON b.id=d.brain_id AND b.user_id=d.user_id AND b.path=d.repo_path";

fn row_to_record(r: &rusqlite::Row) -> rusqlite::Result<DriveRecord> {
    Ok(DriveRecord { id:r.get(0)?,user_id:r.get(1)?,name:r.get(2)?,brain_id:r.get(3)?,repo_path:PathBuf::from(r.get::<_,String>(4)?),branch:r.get(5)?,
        indexed_revision:r.get(6)?,dirty_revision:r.get(7)?,imported_at:r.get(8)?,import_revision:r.get(9)?,kind:r.get(10)?,scope_id:r.get(11)? })
}

fn record(conn: &Connection, owner: &str) -> Result<Option<DriveRecord>> {
    record_kind(conn, "personal", owner)
}

/// Registry row for any drive kind; `scope_id` is the owner for personal
/// drives, the workspace/project/bot/swarm id otherwise.
pub fn record_kind(conn: &Connection, kind: &str, scope_id: &str) -> Result<Option<DriveRecord>> {
    Ok(conn.query_row(&format!("{RECORD_SQL} WHERE d.kind=?1 AND d.scope_id=?2"), params![kind, scope_id], row_to_record).optional()?)
}

pub fn record_by_brain(conn: &Connection, brain_id: &str) -> Result<Option<DriveRecord>> {
    Ok(conn.query_row(&format!("{RECORD_SQL} WHERE d.brain_id=?1"), params![brain_id], row_to_record).optional()?)
}

/// Marker owner of the Memory Drive stored in this brain, if it is one.
pub fn drive_owner_for_brain(conn: &Connection, brain_id: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT user_id FROM memory_drives WHERE brain_id=?1", params![brain_id], |r| r.get(0)).optional()?)
}

/// DbHandle-only writers resolve this durable pointer, never a global root.
pub fn resolve(db: &DbHandle, owner: &str) -> Result<DriveRecord> {
    record(&db.connect()?, owner)?.ok_or(ServiceError::NotFound)
}

/// Personal drive, provisioned on first use.
pub fn provision(db: &DbHandle, brains_dir: &Path, owner: &str) -> Result<DriveRecord> {
    let drive = provision_kind(db, brains_dir, owner, "personal", owner)?;
    repair_index(db, owner)?;
    resolve(db, owner).or(Ok(drive))
}

/// Immediate transaction serializes registry creation, including the owner
/// collision check. A random physical brain id retains the existing smart-HTTP
/// layout. On rollback, remove only the newly generated unpublished repo.
/// `owner` is the marker owner: the account that owns the scope (the user for
/// personal drives, the workspace/project/bot owner otherwise).
pub fn provision_kind(db: &DbHandle, brains_dir: &Path, owner: &str, kind: &str, scope_id: &str) -> Result<DriveRecord> {
    let key = directory_key(owner)?;
    let mut conn = db.connect()?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) = record_kind(&tx, kind, scope_id)? {
        let expected = brains_dir.join(directory_key(&existing.user_id)?).join(format!("{}.git", existing.brain_id));
        if expected != existing.repo_path {
            return Err(ServiceError::Provenance("configured root differs from stored drive; explicit migration required".into()));
        }
        existing.storage()?.initialize()?;
        tx.commit()?;
        return Ok(existing);
    }
    // A malformed/foreign registry cannot be silently replaced.
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM memory_drives WHERE kind=?1 AND scope_id=?2)", params![kind, scope_id], |r|r.get(0))?;
    if exists { return Err(ServiceError::NotFound); }
    let parent = brains_dir.join(&key);
    {
        let mut stmt = tx.prepare("SELECT user_id,path FROM brains WHERE user_id<>?1")?;
        for row in stmt.query_map(params![owner], |r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
            let (other, path) = row?;
            if Path::new(&path).parent() == Some(parent.as_path()) || directory_key(&other).map(|k| k == key).unwrap_or(false) {
                return Err(ServiceError::OwnerCollision);
            }
        }
    }
    let collision: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM memory_drives WHERE directory_key=?1 AND user_id<>?2)", params![key,owner], |r|r.get(0))?;
    if collision { return Err(ServiceError::OwnerCollision); }
    let brain_id = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let path = parent.join(format!("{brain_id}.git"));
    let drive = MemoryDrive::new(path.clone(), owner, "main")?;
    drive.initialize()?;
    let registered = (|| -> Result<()> {
        tx.execute("INSERT INTO brains(id,user_id,path) VALUES(?1,?2,?3)", params![brain_id,owner,path.to_string_lossy()])?;
        tx.execute("INSERT INTO memory_drives(id,user_id,kind,scope_id,directory_key,brain_id,repo_path,dirty_revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id,owner,kind,scope_id,key,brain_id,path.to_string_lossy(),(kind=="personal").then_some("pending")])?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = registered {
        // Inspect after rollback before removing our UUID path. A successful
        // durable registration must survive even an ambiguous commit error.
        if record_kind(&db.connect()?, kind, scope_id)?.is_none() { let _ = std::fs::remove_dir_all(&path); }
        return Err(error);
    }
    record_kind(&db.connect()?, kind, scope_id)?.ok_or(ServiceError::NotFound)
}

/// Re-sync the index after an accepted push. Only personal drives are
/// indexed into recall; shared drives are read from git directly.
pub fn repair_index_for_brain(db: &DbHandle, brain_id: &str) -> Result<()> {
    let Some(drive) = record_by_brain(&db.connect()?, brain_id)? else { return Ok(()) };
    if drive.kind == "personal" { repair_index(db, &drive.user_id)?; }
    Ok(())
}

/// Access of `user` to the shared drive in `brain_id` ("read"/"write").
pub fn member_access(conn: &Connection, user: &str, brain_id: &str) -> Result<Option<String>> {
    let Some(drive) = record_by_brain(conn, brain_id)? else { return Ok(None) };
    Ok(crate::memory_drive_scopes::access(conn, user, &drive.kind, &drive.scope_id)?.map(str::to_string))
}

pub fn entries(snapshot: &Snapshot) -> Result<Vec<(String, Entry)>> {
    let mut entries = Vec::new();
    for (path, content) in &snapshot.files {
        for line in content.lines().filter(|l|l.starts_with("- ") && !l.starts_with("- [[")) {
            entries.push((path.clone(), Entry::parse(line)?));
        }
    }
    Ok(entries)
}

fn known_session(conn: &Connection, owner: &str, session: &str) -> Result<bool> {
    if session.is_empty() || session.len()>128 || !session.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'-' || b==b'_') {
        return Ok(false);
    }
    // Incognito turns must never become personal drive provenance.
    Ok(conn.query_row(
        // agent_sessions has no owner column: a data-plane node serves one
        // account's sessions. beta_sessions are per user.
        "SELECT (EXISTS(SELECT 1 FROM agent_sessions WHERE id=?1 AND ?2 IS NOT NULL)
                 OR EXISTS(SELECT 1 FROM beta_sessions WHERE id=?1 AND user_id=?2))
                AND NOT EXISTS(SELECT 1 FROM ephemeral_sessions WHERE session_id=?1)",
        params![session,owner], |r|r.get(0),
    )?)
}

pub fn session_is_owned(conn: &Connection, owner: &str, session: &str) -> Result<bool> { known_session(conn, owner, session) }
pub fn check_provenance(conn: &Connection, owner: &str, entry: &Entry) -> Result<()> { validate_provenance(conn, owner, entry) }

fn validate_provenance(conn: &Connection, owner: &str, entry: &Entry) -> Result<()> {
    if let Some(obs) = entry.metadata.get("observation") {
        let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM memory_observations WHERE id=?1 AND user_id=?2)",params![obs,owner],|r|r.get(0))?;
        if !valid { return Err(ServiceError::Provenance("observation is not owned by this account".into())); }
    }
    let parsed = url::Url::parse(&entry.source).or_else(|_|url::Url::parse("https://ai.allternit.com/")?.join(&entry.source)).ok();
    let mut sessions = BTreeSet::new();
    if let Some(session) = entry.metadata.get("session") { sessions.insert(session.clone()); }
    if let Some(url) = parsed {
        for (key,value) in url.query_pairs() { if key=="session" { sessions.insert(value.into_owned()); } }
        let path = url.path();
        for prefix in ["/api/v1/agent-sessions/", "/api/v1/beta/sessions/"] {
            if let Some(tail)=path.strip_prefix(prefix) { sessions.insert(tail.split('/').next().unwrap_or_default().to_string()); }
        }
    }
    for session in sessions {
        if !known_session(conn,owner,&session)? { return Err(ServiceError::Provenance("session is missing, foreign or incognito".into())); }
    }
    if let Some(agent)=entry.metadata.get("agent") {
        // An owned observation or session can attest the agent assignment.
        let owned: bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM memory_observations WHERE user_id=?1 AND agent_id=?2)
            OR EXISTS(SELECT 1 FROM agents WHERE user_id=?1 AND id=?2)",params![owner,agent],|r|r.get(0))?;
        if !owned { return Err(ServiceError::Provenance("agent provenance is not owned by this account".into())); }
    }
    if let Some(kind)=entry.metadata.get("memory_type") {
        if !["fact","preference","event","procedure","entity","relationship","task_state","not_memory"].contains(&kind.as_str()) {
            return Err(ServiceError::Provenance("unknown memory_type".into()));
        }
    }
    Ok(())
}

fn active(entry: &Entry) -> bool {
    !matches!(entry.metadata.get("status").map(String::as_str),Some("proposed"|"rejected"))
        && entry.metadata.get("memory_type").map(String::as_str)!=Some("not_memory")
        && !entry.metadata.get("scope").map(|s|s.starts_with("twin")).unwrap_or(false)
        && !matches!(entry.metadata.get("kind").map(String::as_str),Some("question"|"answer"))
}
fn digest(text: &str) -> String { hex::encode(Sha256::digest(text.as_bytes())) }

/// Transactional whole-tree projection. Stable entry identity keeps fact IDs,
/// retrieval counters and graph references across file moves and corrections.
/// No kernel canonical writer is called: insert/update here is an index only.
fn index_snapshot(conn: &Connection, drive: &DriveRecord, snapshot: &Snapshot) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (path, entry) in entries(snapshot)? {
        validate_provenance(conn,&drive.user_id,&entry)?;
        if !active(&entry) { continue; }
        seen.insert(entry.id.clone());
        let hash=digest(&entry.render()?);
        let previous:Option<(String,String)>=conn.query_row("SELECT fact_id,content_hash FROM memory_drive_entries WHERE drive_id=?1 AND entry_id=?2",params![drive.id,entry.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let fact_id=previous.as_ref().map(|p|p.0.clone()).unwrap_or_else(||format!("drive_{}",digest(&format!("{}\0{}",drive.id,entry.id))));
        let intact:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM memory_facts WHERE id=?1 AND user_id=?2 AND fact=?3 AND valid_until IS NULL)",params![fact_id,drive.user_id,entry.text],|r|r.get(0))?;
        if !intact || previous.as_ref().map(|p|p.1.as_str())!=Some(hash.as_str()) {
            let confidence=entry.metadata.get("confidence").and_then(|c|c.parse::<f64>().ok()).filter(|n|n.is_finite() && (0.0..=1.0).contains(n)).unwrap_or(0.85);
            conn.execute("INSERT INTO memory_facts(id,user_id,agent_id,fact,confidence,valid_from,source_observation_id,memory_type)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET agent_id=excluded.agent_id,fact=excluded.fact,confidence=excluded.confidence,
                source_observation_id=excluded.source_observation_id,memory_type=excluded.memory_type,valid_until=NULL",
                params![fact_id,drive.user_id,entry.metadata.get("agent"),entry.text,confidence,entry.added,entry.metadata.get("observation"),entry.metadata.get("memory_type").map(String::as_str).unwrap_or("fact")])?;
            let embedded=memory_index::hash_embed(std::slice::from_ref(&entry.text));
            memory_index::upsert_embedding(conn,&drive.user_id,"fact",&fact_id,&embedded.model,&embedded.vectors[0])?;
        }
        let metadata=serde_json::to_string(&entry.metadata).map_err(|_|ServiceError::Provenance("invalid metadata".into()))?;
        conn.execute("INSERT INTO memory_drive_entries(drive_id,file_path,entry_id,fact_id,content_hash,source,metadata) VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(drive_id,entry_id) DO UPDATE SET file_path=excluded.file_path,content_hash=excluded.content_hash,source=excluded.source,metadata=excluded.metadata",
             params![drive.id,path,entry.id,fact_id,hash,entry.source,metadata])?;
    }
    let old:Vec<(String,String)>={ let mut stmt=conn.prepare("SELECT entry_id,fact_id FROM memory_drive_entries WHERE drive_id=?1")?;
        let rows=stmt.query_map(params![drive.id],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?; rows };
    // A removed entry is retired softly (valid_until), like the kernel's own
    // supersession, so relation edges and undo keep a stable target. Git
    // history is the record; the row is only an index.
    for (id,fact) in old { if !seen.contains(&id) {
        conn.execute("DELETE FROM memory_drive_entries WHERE drive_id=?1 AND entry_id=?2",params![drive.id,id])?;
        crate::memory_relations::supersede_fact(conn,&drive.user_id,&fact)?;
    }}
    recover_import(conn,drive,snapshot)?;
    conn.execute("UPDATE memory_drives SET indexed_revision=?2,dirty_revision=NULL WHERE id=?1",params![drive.id,snapshot.revision])?;
    Ok(())
}

/// Read-side repair also detects a commit whose dirty flag update was lost.
/// Capture the snapshot AFTER taking the projection transaction, so a delayed
/// repair cannot overwrite a newer transaction with an older prepared snapshot.
pub fn reindex(db: &DbHandle, owner: &str) -> Result<DriveRecord> {
    let mut conn=db.connect()?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let drive=record(&tx,owner)?.ok_or(ServiceError::NotFound)?;
    let snapshot=drive.storage()?.snapshot(None)?;
    index_snapshot(&tx,&drive,&snapshot)?;
    tx.commit()?;
    resolve(db,owner)
}
pub fn repair_index(db: &DbHandle, owner: &str) -> Result<()> {
    let Some(drive)=record(&db.connect()?,owner)? else { return Ok(()); };
    if drive.dirty_revision.is_some() || drive.storage()?.head()? != drive.indexed_revision { reindex(db,owner)?; }
    Ok(())
}
pub fn read(db: &DbHandle, owner: &str, revision: Option<&str>) -> Result<Snapshot> {
    repair_index(db,owner)?;
    Ok(resolve(db,owner)?.storage()?.snapshot(revision)?)
}
pub fn apply(db: &DbHandle, owner: &str, expected: &str, operations: &[Operation], message: &str) -> Result<ApplyResult> {
    crate::memory_drive_scopes::guard_managed_paths(operations)?;
    apply_as(db, owner, expected, operations, message, "Agent via Allternit")
}

/// Commit as a named author (e.g. "Dream via Allternit"). Callers other than
/// `apply` are trusted server components (Dream, twin projector).
pub fn apply_as(db: &DbHandle, owner: &str, expected: &str, operations: &[Operation], message: &str, author: &str) -> Result<ApplyResult> {
    let drive=resolve(db,owner)?;
    // Validate complete candidate provenance before publishing, including raw
    // edits. Core validates format/size/secrets and executes the final CAS.
    let mut candidate=drive.storage()?.snapshot(Some(expected))?;
    for op in operations {
        match op {
            Operation::SetFile{content,..} => {
                for line in content.lines().filter(|l|l.starts_with("- ") && !l.starts_with("- [[")) { validate_provenance(&db.connect()?,owner,&Entry::parse(line)?)?; }
            }
            Operation::UpsertEntry{entry,..} => validate_provenance(&db.connect()?,owner,entry)?,
            _=>{}
        }
    }
    for (_,entry) in entries(&candidate)? { validate_provenance(&db.connect()?,owner,&entry)?; }
    candidate.files.clear(); // validation snapshot does not participate in writes
    db.connect()?.execute("UPDATE memory_drives SET dirty_revision='pending' WHERE id=?1 AND user_id=?2",params![drive.id,owner])?;
    let result=drive.storage()?.apply_batch(Some(expected),operations,message,author)?;
    if reindex(db,owner).is_err() { return Err(ServiceError::IndexPending{revision:result.revision}); }
    Ok(result)
}

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct ImportRow {
    pub legacy_id:String,
    pub path:Option<String>,
    pub entry:Option<Entry>,
    pub skipped:Option<String>,
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct ImportPlan {
    pub revision:Option<String>,
    pub already_imported:bool,
    pub rows:Vec<ImportRow>,
    pub total:usize,
    pub converted:usize,
    pub skipped:usize,
    pub topic_files:Vec<String>,
}

pub fn import_plan(db:&DbHandle,owner:&str)->Result<ImportPlan> {
    let conn=db.connect()?;
    let existing=record(&conn,owner)?;
    let revision=existing.as_ref().map(|d|d.storage()?.head().map_err(ServiceError::from)).transpose()?.flatten();
    let already_imported=existing.as_ref().map(|d|d.imported_at.is_some()).unwrap_or(false)
        || existing.as_ref().map(|d|d.storage()?.snapshot(None).map(|s|s.files.contains_key(RECEIPT)).map_err(ServiceError::from)).transpose()?.unwrap_or(false);
    if already_imported { return Ok(ImportPlan{revision,already_imported,rows:vec![],total:0,converted:0,skipped:0,topic_files:vec![]}); }
    let mut stmt=conn.prepare("SELECT f.id,f.fact,f.valid_from,f.agent_id,f.source_observation_id,f.memory_type,f.confidence
        FROM memory_facts f WHERE f.user_id=?1 AND f.valid_until IS NULL
        AND NOT EXISTS(SELECT 1 FROM memory_drive_entries e WHERE e.fact_id=f.id) ORDER BY f.id LIMIT ?2")?;
    let facts=stmt.query_map(params![owner,(MAX_IMPORT_ROWS+1) as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,f64>(6)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
    if facts.len()>MAX_IMPORT_ROWS { return Err(ServiceError::ImportLimit); }
    let mut rows=vec![];
    let mut topics=BTreeSet::new();
    for (id,text,date,agent,observation,kind,confidence) in facts {
        let mut entry=Entry{id:format!("legacy-{}",digest(&id)),text,source:"imported:unknown".into(),added:date.get(..10).unwrap_or("").to_string(),metadata:BTreeMap::new()};
        entry.metadata.insert("origin".into(),"kernel-v1".into());
        // Original id is held in the receipt as its digest, preventing unsafe
        // old IDs from injecting metadata and supporting crash recovery.
        entry.metadata.insert("origin_hash".into(),digest(&id));
        entry.metadata.insert("confidence".into(),confidence.to_string());
        if let Some(kind)=kind { entry.metadata.insert("memory_type".into(),kind); }
        if let Some(agent)=agent { entry.metadata.insert("agent".into(),agent); }
        let mut provenance_error=None;
        if let Some(obs)=observation {
            let row:Option<(Option<String>,String,Option<String>)>=conn.query_row("SELECT session_id,kind,source FROM memory_observations WHERE id=?1 AND user_id=?2",params![obs,owner],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            match row {
                None=>provenance_error=Some("invalid observation provenance".to_string()),
                Some((session,kind,source))=>{
                    if format!("{kind} {}",source.unwrap_or_default()).contains("twin.proposed") { provenance_error=Some("proposed twin memory".into()); }
                    entry.metadata.insert("observation".into(),obs);
                    if let Some(session)=session {
                        if known_session(&conn,owner,&session)? {
                            // This existing authenticated API resource is real.
                            // Shared workspace source-link routing follows later.
                            entry.source=format!("/api/v1/agent-sessions/{session}");
                            entry.metadata.insert("session".into(),session);
                        } else { provenance_error=Some("missing, foreign or incognito session".into()); }
                    }
                }
            }
        }
        let path=format!("imports/{}.md",entry.metadata.get("memory_type").map(String::as_str).unwrap_or("fact"));
        let skipped=provenance_error.or_else(||match entry.render() { Ok(_)=>None,Err(DriveError::Secret)=>Some("possible secret".into()),Err(_)=>Some("invalid entry format".into()) })
            .or_else(||validate_provenance(&conn,owner,&entry).err().map(|_|"invalid provenance or type".to_string()))
            .or_else(||(!active(&entry)).then(||"inactive memory".into()));
        if skipped.is_none() { topics.insert(path.clone()); }
        rows.push(ImportRow{legacy_id:id,path:skipped.is_none().then_some(path),entry:skipped.is_none().then_some(entry),skipped});
    }
    let converted=rows.iter().filter(|r|r.entry.is_some()).count();
    topics.insert(RECEIPT.into());
    Ok(ImportPlan{revision,already_imported,converted,skipped:rows.len()-converted,total:rows.len(),rows,topic_files:topics.into_iter().collect()})
}

/// Dry run is import_plan only: no lazy provisioning or DB mutation. Applied
/// imports require the initialized HEAD revision returned by drive info.
pub fn import_apply(db:&DbHandle,owner:&str,expected:&str)->Result<ApplyResult> {
    let drive=resolve(db,owner)?;
    let snapshot=drive.storage()?.snapshot(None)?;
    if snapshot.files.contains_key(RECEIPT) {
        // Recovery does not make another git commit; CAS still checks caller.
        if snapshot.revision.as_deref()!=Some(expected) { return Err(DriveError::Conflict{expected:Some(expected.into()),actual:snapshot.revision}.into()); }
        reindex(db,owner)?;
        return Ok(ApplyResult{revision:expected.into(),changed:false});
    }
    let plan=import_plan(db,owner)?;
    let now=chrono::Utc::now().to_rfc3339();
    let mut receipt=format!("# Kernel import\n\n## kernel-v1\n\n### Imported at {now}\n");
    let mut operations=vec![];
    for row in plan.rows {
        let id=row.entry.as_ref().map(|e|e.id.as_str()).unwrap_or("skipped");
        receipt.push_str(&format!("### Row {} {id}\n",digest(&row.legacy_id)));
        if let (Some(path),Some(entry))=(row.path,row.entry) { operations.push(Operation::UpsertEntry{path,entry}); }
    }
    operations.push(Operation::SetFile{path:RECEIPT.into(),content:receipt});
    apply(db,owner,expected,&operations,"Import existing memory")
}

fn recover_import(conn:&Connection,drive:&DriveRecord,snapshot:&Snapshot)->Result<()> {
    if drive.imported_at.is_some() { return Ok(()); }
    let Some(receipt)=snapshot.files.get(RECEIPT) else { return Ok(()); };
    if !receipt.lines().any(|l|l=="## kernel-v1") { return Err(ServiceError::Provenance("invalid import receipt".into())); }
    let at=receipt.lines().find_map(|l|l.strip_prefix("### Imported at ")).ok_or_else(||ServiceError::Provenance("missing import timestamp".into()))?;
    chrono::DateTime::parse_from_rfc3339(at).map_err(|_|ServiceError::Provenance("invalid import timestamp".into()))?;
    let origin: BTreeMap<_,_>=entries(snapshot)?.into_iter().filter(|(_,e)|e.metadata.get("origin").map(String::as_str)==Some("kernel-v1"))
        .filter_map(|(_,e)|e.metadata.get("origin_hash").cloned().map(|h|(h,e.id))).collect();
    let receipt_rows:Vec<(&str,&str)>=receipt.lines().filter_map(|l|l.strip_prefix("### Row ")).map(|l|l.split_once(' ').ok_or_else(||ServiceError::Provenance("invalid import row receipt".into()))).collect::<Result<_>>()?;
    if receipt_rows.len()>MAX_IMPORT_ROWS { return Err(ServiceError::ImportLimit); }
    let mut hashes=BTreeMap::new();
    { let mut stmt=conn.prepare("SELECT id FROM memory_facts WHERE user_id=?1 AND NOT EXISTS(SELECT 1 FROM memory_drive_entries e WHERE e.fact_id=memory_facts.id)")?;
      for row in stmt.query_map(params![drive.user_id],|r|r.get::<_,String>(0))? { let id=row?; hashes.insert(digest(&id),id); } }
    for (hash,id) in receipt_rows {
        if hash.len()!=64 || !hash.bytes().all(|b|b.is_ascii_hexdigit()) || (id!="skipped" && origin.get(hash).map(String::as_str)!=Some(id)) {
            return Err(ServiceError::Provenance("import receipt does not match imported entries".into()));
        }
        let legacy=hashes.get(hash).ok_or_else(||ServiceError::Provenance("import source row missing".into()))?;
        conn.execute("INSERT OR IGNORE INTO memory_drive_import_rows(drive_id,legacy_fact_id,entry_id,imported_at) VALUES(?1,?2,?3,?4)",params![drive.id,legacy,(id!="skipped").then_some(id),at])?;
    }
    conn.execute("UPDATE memory_drives SET imported_at=?2,import_revision=?3 WHERE id=?1",params![drive.id,at,snapshot.revision])?;
    // Converted rows now live in the drive: retire the old copies (kept as
    // archive, never deleted) so recall isn't doubled.
    conn.execute("DELETE FROM memory_embeddings WHERE user_id=?2 AND target_type='fact' AND target_id IN
        (SELECT legacy_fact_id FROM memory_drive_import_rows WHERE drive_id=?1 AND entry_id IS NOT NULL)",params![drive.id,drive.user_id])?;
    conn.execute("UPDATE memory_facts SET valid_until=?3 WHERE user_id=?2 AND valid_until IS NULL AND id IN
        (SELECT legacy_fact_id FROM memory_drive_import_rows WHERE drive_id=?1 AND entry_id IS NOT NULL)",params![drive.id,drive.user_id,at])?;
    Ok(())
}

/// Old rows the import skipped stay visible for 30 days after the import,
/// then are retired (soft; rows are kept). Returns how many were hidden.
pub fn hide_expired_archives(db:&DbHandle)->Result<usize> {
    let conn=db.connect()?;
    conn.execute("DELETE FROM memory_embeddings WHERE target_type='fact' AND target_id IN
        (SELECT i.legacy_fact_id FROM memory_drive_import_rows i JOIN memory_facts f ON f.id=i.legacy_fact_id
         WHERE i.entry_id IS NULL AND f.valid_until IS NULL AND datetime(i.imported_at,'+30 days')<=datetime('now'))",[])?;
    Ok(conn.execute("UPDATE memory_facts SET valid_until=CURRENT_TIMESTAMP WHERE valid_until IS NULL AND id IN
        (SELECT i.legacy_fact_id FROM memory_drive_import_rows i JOIN memory_drives d ON d.id=i.drive_id
         WHERE d.user_id=memory_facts.user_id AND i.entry_id IS NULL AND datetime(i.imported_at,'+30 days')<=datetime('now'))",[])?)
}
