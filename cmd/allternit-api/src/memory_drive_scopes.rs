//! Shared Memory Drives (Phase 3): project, team, bot and swarm drives next to
//! each user's personal drive, the permission matrix checked on every request
//! and token, and the `questions.md` board agents use to ask and answer.
//!
//! Permission matrix (re-evaluated on every call, so removal revokes):
//! - personal:<user>       the user only, write.
//! - team:<workspace>      workspace owner and members write; role `viewer` reads.
//! - project:<project>     the project's owner (cowork_projects.user_id), write.
//! - bot:<agent>           the bot's owner (agents.user_id), write.
//! - swarm:<orchestrator>  the swarm's owner (agents.user_id, type orchestrator), write.
//!
//! Shared drives are separate repos. Nothing is copied between drives and
//! `[[links]]` resolve only inside the drive they are in. Shared drives are
//! not indexed into anyone's personal recall; sessions read them as mounts.
use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::db::DbHandle;
use crate::memory_drive::{DriveError, Entry, Operation};
use crate::memory_drive_service::{self as service, DriveRecord, Result, ServiceError};

pub const QUESTIONS: &str = "questions.md";
const MAX_CAS_RETRIES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveRef {
    pub kind: String,
    pub scope_id: String,
}

impl DriveRef {
    pub fn personal(user: &str) -> Self {
        Self { kind: "personal".into(), scope_id: user.into() }
    }
    /// `personal` | `project:<id>` | `team:<id>` | `bot:<id>` | `swarm:<id>`.
    pub fn parse(raw: Option<&str>, user: &str) -> Result<Self> {
        let raw = raw.map(str::trim).filter(|r| !r.is_empty()).unwrap_or("personal");
        if raw == "personal" {
            return Ok(Self::personal(user));
        }
        let (kind, id) = raw
            .split_once(':')
            .ok_or_else(|| ServiceError::Provenance("drive must be personal or kind:id".into()))?;
        if !matches!(kind, "project" | "team" | "bot" | "swarm")
            || id.is_empty()
            || id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        {
            return Err(ServiceError::Provenance("unknown drive".into()));
        }
        Ok(Self { kind: kind.into(), scope_id: id.into() })
    }
    pub fn label(&self) -> String {
        if self.kind == "personal" {
            "personal".into()
        } else {
            format!("{}:{}", self.kind, self.scope_id)
        }
    }
}

/// The account that owns a scope (and is the drive's marker owner).
pub fn scope_owner(conn: &Connection, kind: &str, scope_id: &str) -> rusqlite::Result<Option<String>> {
    let sql = match kind {
        "personal" => return Ok(Some(scope_id.to_string())),
        "team" => "SELECT owner_id FROM workspaces WHERE id=?1",
        "project" => "SELECT user_id FROM cowork_projects WHERE id=?1",
        "bot" => "SELECT user_id FROM agents WHERE id=?1",
        "swarm" => "SELECT user_id FROM agents WHERE id=?1 AND type='orchestrator'",
        _ => return Ok(None),
    };
    conn.query_row(sql, params![scope_id], |r| r.get(0)).optional()
}

/// `Some("write")`, `Some("read")` or `None` (no access).
pub fn access(conn: &Connection, user: &str, kind: &str, scope_id: &str) -> rusqlite::Result<Option<&'static str>> {
    if user.is_empty() {
        return Ok(None);
    }
    if kind == "team" {
        if scope_owner(conn, kind, scope_id)?.as_deref() == Some(user) {
            return Ok(Some("write"));
        }
        let role: Option<String> = conn
            .query_row(
                "SELECT role FROM workspace_members WHERE workspace_id=?1 AND user_id=?2 LIMIT 1",
                params![scope_id, user],
                |r| r.get(0),
            )
            .optional()?;
        return Ok(role.map(|r| if r == "viewer" { "read" } else { "write" }));
    }
    Ok((scope_owner(conn, kind, scope_id)?.as_deref() == Some(user)).then_some("write"))
}

/// Open a drive for `user`, checking access and provisioning it on first
/// use. `need_write` refuses read-only members.
pub fn open(db: &DbHandle, root: &Path, user: &str, drive: &DriveRef, need_write: bool) -> Result<DriveRecord> {
    let conn = db.connect()?;
    let Some(level) = access(&conn, user, &drive.kind, &drive.scope_id)? else {
        return Err(ServiceError::Forbidden);
    };
    if need_write && level != "write" {
        return Err(ServiceError::Forbidden);
    }
    let owner = scope_owner(&conn, &drive.kind, &drive.scope_id)?.ok_or(ServiceError::NotFound)?;
    drop(conn);
    if drive.kind == "personal" {
        return service::provision(db, root, user);
    }
    let d = match service::record_kind(&db.connect()?, &drive.kind, &drive.scope_id)? {
        Some(d) => d,
        None => service::provision_kind(db, root, &owner, &drive.kind, &drive.scope_id)?,
    };
    if matches!(d.kind.as_str(), "bot" | "project") {
        if let Err(e) = sync_cowork_mirror(db, &d) {
            tracing::warn!("cowork memory mirror for a {} drive failed: {e}", d.kind);
        }
    }
    Ok(d)
}

/// Bot and project memory written by the cowork runtime (principal-scoped
/// `cowork_memory_entries`) is mirrored read-only into `cowork/memory.md` of
/// the bot or project drive, so it is visible, versioned and clonable with
/// the rest of that drive. The cowork store keeps its grants model and stays
/// the writer for those entries (Phase 4 inventory: compatibility facade).
pub fn sync_cowork_mirror(db: &DbHandle, drive: &DriveRecord) -> Result<()> {
    let conn = db.connect()?;
    let (sql, arg) = match drive.kind.as_str() {
        "bot" => (
            "SELECT e.id,e.content,e.type,COALESCE(e.created_at,''),e.session_id FROM cowork_memory_entries e
             WHERE e.user_id=?1 AND (e.owner_principal = 'a://local/bot/' || ?2
                OR e.owner_principal = (SELECT principal_id FROM agents WHERE id=?2 AND user_id=?1))
             ORDER BY e.created_at, e.id LIMIT 400",
            drive.scope_id.clone(),
        ),
        "project" => (
            "SELECT e.id,e.content,e.type,COALESCE(e.created_at,''),e.session_id FROM cowork_memory_entries e
             WHERE e.user_id=?1 AND e.project_id=?2 ORDER BY e.created_at, e.id LIMIT 400",
            drive.scope_id.clone(),
        ),
        _ => return Ok(()),
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![drive.user_id, arg], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut body = String::new();
    for (id, content, kind, created, session) in rows {
        let text = content.split_whitespace().collect::<Vec<_>>().join(" ");
        let safe_id: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(100).collect();
        if text.is_empty() || safe_id.is_empty() || text.len() > 4000 {
            continue;
        }
        let mut e = Entry {
            id: format!("cw-{safe_id}"),
            text,
            source: session.filter(|s| s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).map(|s| format!("/?session={s}")).unwrap_or_else(|| format!("allternit:cowork/{safe_id}")),
            added: created.get(..10).filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()).unwrap_or("1970-01-01").to_string(),
            metadata: Default::default(),
        };
        e.metadata.insert("memory_type".into(), if crate::memory_relations::MemoryType::parse(&kind).is_some() { kind } else { "fact".into() });
        e.metadata.insert("origin".into(), "cowork".into());
        // Skip lines that would fail the drive's format or secret checks.
        if let Ok(line) = e.render() {
            body.push_str(&line);
            body.push('\n');
        }
    }
    let content = (!body.is_empty()).then(|| format!("# {} memory\n\n## Mirrored by Allternit from the {}; change it there\n\n{body}", if drive.kind == "bot" { "Bot" } else { "Project" }, drive.kind));
    for _ in 0..4 {
        let storage = drive.storage()?;
        let Some(head) = storage.head()? else { return Ok(()) };
        let current = storage.snapshot(Some(&head))?.files.remove("cowork/memory.md");
        let op = match (&content, &current) {
            (Some(c), cur) if cur.as_ref() != Some(c) => Operation::SetFile { path: "cowork/memory.md".into(), content: c.clone() },
            (None, Some(_)) => Operation::DeleteFile { path: "cowork/memory.md".into() },
            _ => return Ok(()),
        };
        match storage.apply_batch(Some(&head), &[op], "Sync cowork memory", "Allternit") {
            Ok(_) => return Ok(()),
            Err(DriveError::Conflict { .. }) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Read access only, without provisioning (used for listing).
pub fn existing(db: &DbHandle, user: &str, drive: &DriveRef) -> Result<Option<DriveRecord>> {
    let conn = db.connect()?;
    if access(&conn, user, &drive.kind, &drive.scope_id)?.is_none() {
        return Err(ServiceError::Forbidden);
    }
    service::record_kind(&conn, &drive.kind, &drive.scope_id)
}

/// Commit to any drive. Personal drives go through the indexed service path;
/// shared drives check the writer's provenance and commit directly.
pub fn apply_for(
    db: &DbHandle,
    user: &str,
    drive: &DriveRecord,
    expected: &str,
    operations: &[Operation],
    message: &str,
) -> Result<crate::memory_drive::ApplyResult> {
    if drive.kind == "personal" {
        return service::apply(db, &drive.user_id, expected, operations, message);
    }
    guard_managed_paths(operations)?;
    let conn = db.connect()?;
    for op in operations {
        match op {
            Operation::UpsertEntry { entry, .. } => service::check_provenance(&conn, user, entry)?,
            Operation::SetFile { content, .. } => {
                for line in content.lines().filter(|l| l.starts_with("- ") && !l.starts_with("- [[")) {
                    service::check_provenance(&conn, user, &Entry::parse(line)?)?;
                }
            }
            _ => {}
        }
    }
    drop(conn);
    Ok(drive.storage()?.apply_batch(Some(expected), operations, message, "Agent via Allternit")?)
}

/// `twin/` is a projection of the owner-approved twin store; nothing but the
/// twin projector may write it (a push or edit can't activate a proposal).
pub fn guard_managed_paths(operations: &[Operation]) -> Result<()> {
    for op in operations {
        let path = match op {
            Operation::SetFile { path, .. } | Operation::DeleteFile { path } | Operation::UpsertEntry { path, .. } => path.as_str(),
            Operation::DeleteEntry { .. } => continue,
        };
        if path.starts_with("twin/") {
            return Err(ServiceError::Provenance(
                "twin/ is managed by Allternit; change twin memories in Settings → Twin".into(),
            ));
        }
        if path.starts_with("cowork/") {
            return Err(ServiceError::Provenance("cowork/ mirrors bot and project memory; change it in the bot or project".into()));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Mount {
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: String,
    pub name: String,
    pub access: String,
    pub revision: Option<String>,
}

/// Every drive the user can open: personal first, then existing shared
/// drives they have access to, then scopes they could open (not yet created).
pub fn mounts(db: &DbHandle, user: &str) -> Result<Vec<Mount>> {
    let conn = db.connect()?;
    let mut out = Vec::new();
    let personal = service::record_kind(&conn, "personal", user)?;
    out.push(Mount {
        reference: "personal".into(),
        kind: "personal".into(),
        name: "Personal".into(),
        access: "write".into(),
        revision: personal.and_then(|d| d.storage().ok()?.head().ok().flatten()),
    });
    let mut candidates: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut add = |sql: &str, kind: &str| -> rusqlite::Result<()> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![user], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        for row in rows {
            let (id, name) = row?;
            candidates.insert((kind.to_string(), id.clone()), name.unwrap_or(id));
        }
        Ok(())
    };
    add("SELECT w.id,w.name FROM workspaces w WHERE w.owner_id=?1
         UNION SELECT w.id,w.name FROM workspaces w JOIN workspace_members m ON m.workspace_id=w.id WHERE m.user_id=?1", "team")?;
    add("SELECT id,title FROM cowork_projects WHERE user_id=?1", "project")?;
    add("SELECT id,name FROM agents WHERE user_id=?1 AND type='orchestrator'", "swarm")?;
    add("SELECT id,name FROM agents WHERE user_id=?1 AND type<>'orchestrator'", "bot")?;
    for ((kind, id), name) in candidates {
        let Some(level) = access(&conn, user, &kind, &id)? else { continue };
        let revision = service::record_kind(&conn, &kind, &id)?.and_then(|d| d.storage().ok()?.head().ok().flatten());
        out.push(Mount { reference: format!("{kind}:{id}"), kind, name, access: level.into(), revision });
    }
    Ok(out)
}

// ─── questions.md board ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    pub id: String,
    pub text: String,
    pub author: String,
    pub added: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Question {
    pub id: String,
    pub text: String,
    pub author: String,
    pub added: String,
    pub status: String,
    pub source: String,
    pub answers: Vec<Answer>,
}

fn board_entries(drive: &DriveRecord) -> Result<(String, Vec<Entry>)> {
    let storage = drive.storage()?;
    let head = storage.head()?.ok_or(DriveError::Uninitialized)?;
    let snapshot = storage.snapshot(Some(&head))?;
    let mut entries = Vec::new();
    if let Some(content) = snapshot.files.get(QUESTIONS) {
        for line in content.lines().filter(|l| l.starts_with("- ") && !l.starts_with("- [[")) {
            entries.push(Entry::parse(line)?);
        }
    }
    Ok((head, entries))
}

pub fn questions(drive: &DriveRecord) -> Result<(String, Vec<Question>)> {
    let (head, entries) = board_entries(drive)?;
    let mut questions: Vec<Question> = entries
        .iter()
        .filter(|e| e.metadata.get("kind").map(String::as_str) == Some("question"))
        .map(|e| Question {
            id: e.id.clone(),
            text: e.text.clone(),
            author: e.metadata.get("author").cloned().unwrap_or_default(),
            added: e.added.clone(),
            status: e.metadata.get("status").cloned().unwrap_or_else(|| "open".into()),
            source: e.source.clone(),
            answers: vec![],
        })
        .collect();
    for e in entries.iter().filter(|e| e.metadata.get("kind").map(String::as_str) == Some("answer")) {
        if let Some(q) = questions.iter_mut().find(|q| Some(&q.id) == e.metadata.get("parent")) {
            q.answers.push(Answer {
                id: e.id.clone(),
                text: e.text.clone(),
                author: e.metadata.get("author").cloned().unwrap_or_default(),
                added: e.added.clone(),
                source: e.source.clone(),
            });
            if q.status == "open" {
                q.status = "answered".into();
            }
        }
    }
    Ok((head, questions))
}

fn author_label(author: &str) -> String {
    let cleaned: String = author
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ';' | '[' | ']' | '\\'))
        .take(80)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() { "member".into() } else { cleaned }
}

/// Apply one board change against the latest head, retrying on concurrent
/// writers. Entries carry stable ids, so a retry never duplicates or loses
/// another writer's question or answer.
fn board_commit(
    db: &DbHandle,
    user: &str,
    drive: &DriveRecord,
    message: &str,
    build: impl Fn(&[Entry]) -> Result<Vec<Operation>>,
) -> Result<String> {
    for attempt in 0..=MAX_CAS_RETRIES {
        let (head, entries) = board_entries(drive)?;
        let ops = build(&entries)?;
        match apply_for(db, user, drive, &head, &ops, message) {
            Ok(r) => return Ok(r.revision),
            Err(ServiceError::Drive(DriveError::Conflict { .. })) if attempt < MAX_CAS_RETRIES => continue,
            Err(e) => return Err(e),
        }
    }
    Err(ServiceError::Drive(DriveError::Conflict { expected: None, actual: None }))
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn default_source(drive: &DriveRecord) -> String {
    format!("allternit:drive/{}", drive.id)
}

pub fn ask(db: &DbHandle, user: &str, author: &str, drive: &DriveRecord, text: &str, source: Option<&str>) -> Result<(String, Entry)> {
    let mut entry = Entry {
        id: format!("q-{}", uuid::Uuid::new_v4().simple()),
        text: text.split_whitespace().collect::<Vec<_>>().join(" "),
        source: source.map(str::to_string).unwrap_or_else(|| default_source(drive)),
        added: today(),
        metadata: Default::default(),
    };
    entry.metadata.insert("kind".into(), "question".into());
    entry.metadata.insert("status".into(), "open".into());
    entry.metadata.insert("author".into(), author_label(author));
    entry.render()?;
    let e = entry.clone();
    let rev = board_commit(db, user, drive, "Ask a question", |_| {
        Ok(vec![Operation::UpsertEntry { path: QUESTIONS.into(), entry: e.clone() }])
    })?;
    Ok((rev, entry))
}

pub fn answer(db: &DbHandle, user: &str, author: &str, drive: &DriveRecord, question: &str, text: &str, source: Option<&str>) -> Result<(String, Entry)> {
    let mut entry = Entry {
        id: format!("a-{}", uuid::Uuid::new_v4().simple()),
        text: text.split_whitespace().collect::<Vec<_>>().join(" "),
        source: source.map(str::to_string).unwrap_or_else(|| default_source(drive)),
        added: today(),
        metadata: Default::default(),
    };
    entry.metadata.insert("kind".into(), "answer".into());
    entry.metadata.insert("parent".into(), question.to_string());
    entry.metadata.insert("author".into(), author_label(author));
    entry.render()?;
    let e = entry.clone();
    let q = question.to_string();
    let rev = board_commit(db, user, drive, "Answer a question", |entries| {
        if !entries.iter().any(|x| x.id == q && x.metadata.get("kind").map(String::as_str) == Some("question")) {
            return Err(DriveError::NotFound(q.clone()).into());
        }
        Ok(vec![Operation::UpsertEntry { path: QUESTIONS.into(), entry: e.clone() }])
    })?;
    Ok((rev, entry))
}

pub fn resolve(db: &DbHandle, user: &str, drive: &DriveRecord, question: &str) -> Result<String> {
    let q = question.to_string();
    board_commit(db, user, drive, "Resolve a question", |entries| {
        let mut entry = entries
            .iter()
            .find(|x| x.id == q && x.metadata.get("kind").map(String::as_str) == Some("question"))
            .cloned()
            .ok_or_else(|| DriveError::NotFound(q.clone()))?;
        entry.metadata.insert("status".into(), "resolved".into());
        Ok(vec![Operation::UpsertEntry { path: QUESTIONS.into(), entry }])
    })
}
