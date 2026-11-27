//! Knowledge factory store (contract: `handoffs/memory-raw-rag-knowledge-factory-2026-11-27/CONTRACT.md`,
//! section "Knowledge factory"; tables from `V298__knowledge_factory.sql`).
//!
//! - **Flags**: an agent could not find knowledge it needed, or found knowledge
//!   that is wrong or stale. Agents file them freely (`open`).
//! - **Proposals**: a fix for a skill, a domain note or a data-access guide,
//!   written by the curator, an agent or skill growth. Every proposal waits for
//!   the person: nothing here approves or activates on its own.
//!
//! Linking: a proposal that cites an `open` flag in its evidence moves that
//! flag to `proposed` with `proposal_id` set. Approving or rejecting the
//! proposal moves its linked flags to `resolved`. Dismissing a flag leaves any
//! proposal alone.
//!
//! Every function takes the owner's `user_id` and every query is filtered by
//! it: one user never reads or changes another's rows.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::DbHandle;

pub const FLAG_KINDS: &[&str] = &["missing", "wrong", "stale"];
pub const TARGET_KINDS: &[&str] = &["skill", "domain", "data_access"];
pub const FLAG_STATUSES: &[&str] = &["open", "proposed", "resolved", "dismissed"];
pub const PROPOSAL_STATUSES: &[&str] = &["proposed", "approved", "rejected", "active", "rolled_back"];
pub const ORIGINS: &[&str] = &["curator", "agent", "skill_growth"];

const MAX_EVIDENCE_CHARS: usize = 8_000;
const MAX_FIX_CHARS: usize = 8_000;
const MAX_TITLE_CHARS: usize = 200;
const MAX_BODY_CHARS: usize = 64_000;
const MAX_REF_CHARS: usize = 128;
const MAX_ID_CHARS: usize = 200;
const MAX_EVIDENCE_ITEMS: usize = 50;

#[derive(Debug)]
pub enum StoreError {
    /// Bad input (400).
    Invalid(String),
    /// No such row for this user (404).
    NotFound,
    /// The row is not in a state that allows this change (409).
    Conflict(String),
    Db(rusqlite::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Invalid(m) | StoreError::Conflict(m) => f.write_str(m),
            StoreError::NotFound => f.write_str("not found"),
            StoreError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

// ── Rows ────────────────────────────────────────────────────────────────────

/// One `knowledge_flags` row (the JSON shape is the row columns).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Flag {
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub kind: String,
    pub target_kind: String,
    pub target_ref: Option<String>,
    pub evidence: String,
    pub suggested_fix: Option<String>,
    pub status: String,
    pub proposal_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One cited piece of evidence: exactly one of the two ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_id: Option<String>,
}

/// One `knowledge_proposals` row. `evidence_json` and `eval_json` come back
/// parsed as `evidence` and `eval`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    pub id: String,
    pub user_id: String,
    pub target_kind: String,
    pub target_ref: String,
    pub title: String,
    pub body: String,
    pub base: Option<String>,
    pub evidence: Vec<EvidenceRef>,
    pub origin: String,
    pub eval: Option<Value>,
    pub status: String,
    pub decided_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewFlag {
    pub kind: String,
    pub target_kind: String,
    #[serde(default)]
    pub target_ref: Option<String>,
    pub evidence: String,
    #[serde(default)]
    pub suggested_fix: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewProposal {
    pub target_kind: String,
    pub target_ref: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    pub origin: String,
    #[serde(default)]
    pub eval: Option<Value>,
}

/// The person's (or the client's) decision on a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// `proposed` → `approved`; linked flags → `resolved`.
    Approve,
    /// `proposed` → `rejected`; linked flags → `resolved`.
    Reject,
    /// `approved` → `active` (the client turned it on).
    Activated,
    /// `approved` | `active` → `rolled_back`.
    Rollback,
}

impl Decision {
    fn from_states(self) -> &'static [&'static str] {
        match self {
            Decision::Approve | Decision::Reject => &["proposed"],
            Decision::Activated => &["approved"],
            Decision::Rollback => &["approved", "active"],
        }
    }
    fn to_state(self) -> &'static str {
        match self {
            Decision::Approve => "approved",
            Decision::Reject => "rejected",
            Decision::Activated => "active",
            Decision::Rollback => "rolled_back",
        }
    }
    fn resolves_flags(self) -> bool {
        matches!(self, Decision::Approve | Decision::Reject)
    }
}

// ── Validation ──────────────────────────────────────────────────────────────

fn one_of(field: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!("{field} must be one of {}", allowed.join(", "))))
    }
}

fn required(field: &str, value: &str, max: usize) -> Result<String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(StoreError::Invalid(format!("{field} is required")));
    }
    if v.chars().count() > max {
        return Err(StoreError::Invalid(format!("{field} is longer than {max} characters")));
    }
    Ok(v.to_string())
}

fn optional(field: &str, value: Option<&str>, max: usize) -> Result<Option<String>> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) if v.chars().count() > max => Err(StoreError::Invalid(format!("{field} is longer than {max} characters"))),
        Some(v) => Ok(Some(v.to_string())),
    }
}

/// A proposal's `target_ref`. Domain and data-access refs become a file name
/// (`knowledge/domain/<ref>.md`, `knowledge/data-access/<ref>.md`), so they are
/// a plain slug. Skill refs may also carry `:` and `/` (`plugin:skill`), never `..`.
pub fn valid_target_ref(target_kind: &str, raw: &str) -> Result<String> {
    let r = required("target_ref", raw, MAX_REF_CHARS)?;
    let slug_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.';
    let ok = match target_kind {
        "skill" => r.bytes().all(|b| slug_byte(b) || b == b':' || b == b'/') && !r.starts_with('/'),
        _ => r.bytes().all(slug_byte),
    };
    if !ok || r.starts_with('.') || r.contains("..") {
        return Err(StoreError::Invalid(
            "target_ref may use letters, digits, '-', '_' and '.' (skills also ':' and '/')".into(),
        ));
    }
    Ok(r)
}

// ── Flags ───────────────────────────────────────────────────────────────────

const FLAG_COLS: &str = "id, user_id, agent_id, session_id, turn_id, kind, target_kind, target_ref, evidence,
     suggested_fix, status, proposal_id, created_at, updated_at";

fn flag_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Flag> {
    Ok(Flag {
        id: r.get(0)?,
        user_id: r.get(1)?,
        agent_id: r.get(2)?,
        session_id: r.get(3)?,
        turn_id: r.get(4)?,
        kind: r.get(5)?,
        target_kind: r.get(6)?,
        target_ref: r.get(7)?,
        evidence: r.get(8)?,
        suggested_fix: r.get(9)?,
        status: r.get(10)?,
        proposal_id: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

fn user_required(user_id: &str) -> Result<()> {
    if user_id.trim().is_empty() {
        return Err(StoreError::Invalid("user_id is required".into()));
    }
    Ok(())
}

fn flag_on(conn: &Connection, user_id: &str, id: &str) -> Result<Option<Flag>> {
    let sql = format!("SELECT {FLAG_COLS} FROM knowledge_flags WHERE user_id = ?1 AND id = ?2");
    Ok(conn.query_row(&sql, params![user_id, id], flag_row).optional()?)
}

/// File a flag for `user_id`. Status starts `open`.
pub fn create_flag(db: &DbHandle, user_id: &str, new: &NewFlag) -> Result<Flag> {
    user_required(user_id)?;
    one_of("kind", new.kind.trim(), FLAG_KINDS)?;
    one_of("target_kind", new.target_kind.trim(), TARGET_KINDS)?;
    let evidence = required("evidence", &new.evidence, MAX_EVIDENCE_CHARS)?;
    let target_ref = optional("target_ref", new.target_ref.as_deref(), MAX_REF_CHARS)?;
    let suggested_fix = optional("suggested_fix", new.suggested_fix.as_deref(), MAX_FIX_CHARS)?;
    let session_id = optional("session_id", new.session_id.as_deref(), MAX_ID_CHARS)?;
    let turn_id = optional("turn_id", new.turn_id.as_deref(), MAX_ID_CHARS)?;
    let agent_id = optional("agent_id", new.agent_id.as_deref(), MAX_ID_CHARS)?;
    let id = format!("kflag_{}", uuid::Uuid::new_v4().simple());
    let conn = db.connect()?;
    conn.execute(
        "INSERT INTO knowledge_flags (id, user_id, agent_id, session_id, turn_id, kind, target_kind, target_ref,
             evidence, suggested_fix)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![id, user_id, agent_id, session_id, turn_id, new.kind.trim(), new.target_kind.trim(), target_ref, evidence, suggested_fix],
    )?;
    flag_on(&conn, user_id, &id)?.ok_or(StoreError::NotFound)
}

/// The user's flags, newest first, optionally of one status. `limit` 1–200.
pub fn list_flags(db: &DbHandle, user_id: &str, status: Option<&str>, limit: i64) -> Result<Vec<Flag>> {
    if let Some(s) = status {
        one_of("status", s, FLAG_STATUSES)?;
    }
    let conn = db.connect()?;
    let sql = format!(
        "SELECT {FLAG_COLS} FROM knowledge_flags WHERE user_id = ?1 AND (?2 IS NULL OR status = ?2)
         ORDER BY created_at DESC, rowid DESC LIMIT ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![user_id, status, limit.clamp(1, 200)], flag_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get_flag(db: &DbHandle, user_id: &str, id: &str) -> Result<Option<Flag>> {
    flag_on(&db.connect()?, user_id, id)
}

/// Set a flag's status (`open` | `proposed` | `resolved` | `dismissed`).
pub fn set_flag_status(db: &DbHandle, user_id: &str, id: &str, status: &str) -> Result<Flag> {
    one_of("status", status, FLAG_STATUSES)?;
    let conn = db.connect()?;
    let n = conn.execute(
        "UPDATE knowledge_flags SET status = ?3, updated_at = datetime('now') WHERE user_id = ?1 AND id = ?2",
        params![user_id, id, status],
    )?;
    if n == 0 {
        return Err(StoreError::NotFound);
    }
    flag_on(&conn, user_id, id)?.ok_or(StoreError::NotFound)
}

/// The person dismisses a flag. A resolved flag stays resolved (409); a
/// linked proposal is left alone.
pub fn dismiss_flag(db: &DbHandle, user_id: &str, id: &str) -> Result<Flag> {
    let conn = db.connect()?;
    let n = conn.execute(
        "UPDATE knowledge_flags SET status = 'dismissed', updated_at = datetime('now')
         WHERE user_id = ?1 AND id = ?2 AND status IN ('open', 'proposed', 'dismissed')",
        params![user_id, id],
    )?;
    if n == 0 {
        return match flag_on(&conn, user_id, id)? {
            None => Err(StoreError::NotFound),
            Some(f) => Err(StoreError::Conflict(format!("flag is {}", f.status))),
        };
    }
    flag_on(&conn, user_id, id)?.ok_or(StoreError::NotFound)
}

// ── Proposals ───────────────────────────────────────────────────────────────

const PROPOSAL_COLS: &str = "id, user_id, target_kind, target_ref, title, body, base, evidence_json, origin,
     eval_json, status, decided_at, created_at, updated_at";

fn proposal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
    let evidence_json: String = r.get(7)?;
    let eval_json: Option<String> = r.get(9)?;
    Ok(Proposal {
        id: r.get(0)?,
        user_id: r.get(1)?,
        target_kind: r.get(2)?,
        target_ref: r.get(3)?,
        title: r.get(4)?,
        body: r.get(5)?,
        base: r.get(6)?,
        evidence: serde_json::from_str(&evidence_json).unwrap_or_default(),
        origin: r.get(8)?,
        eval: eval_json.and_then(|s| serde_json::from_str(&s).ok()),
        status: r.get(10)?,
        decided_at: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

fn proposal_on(conn: &Connection, user_id: &str, id: &str) -> Result<Option<Proposal>> {
    let sql = format!("SELECT {PROPOSAL_COLS} FROM knowledge_proposals WHERE user_id = ?1 AND id = ?2");
    Ok(conn.query_row(&sql, params![user_id, id], proposal_row).optional()?)
}

/// Check every cited id belongs to `user_id` and normalise the list.
fn checked_evidence(conn: &Connection, user_id: &str, evidence: &[EvidenceRef]) -> Result<Vec<EvidenceRef>> {
    if evidence.len() > MAX_EVIDENCE_ITEMS {
        return Err(StoreError::Invalid(format!("evidence has more than {MAX_EVIDENCE_ITEMS} items")));
    }
    let mut out = Vec::with_capacity(evidence.len());
    for e in evidence {
        let flag = optional("evidence.flag_id", e.flag_id.as_deref(), MAX_ID_CHARS)?;
        let obs = optional("evidence.observation_id", e.observation_id.as_deref(), MAX_ID_CHARS)?;
        match (&flag, &obs) {
            (Some(f), None) => {
                if flag_on(conn, user_id, f)?.is_none() {
                    return Err(StoreError::Invalid(format!("unknown flag {f}")));
                }
            }
            (None, Some(o)) => {
                let found: Option<i64> = conn
                    .query_row(
                        "SELECT 1 FROM memory_observations WHERE user_id = ?1 AND id = ?2",
                        params![user_id, o],
                        |r| r.get(0),
                    )
                    .optional()?;
                if found.is_none() {
                    return Err(StoreError::Invalid(format!("unknown observation {o}")));
                }
            }
            _ => return Err(StoreError::Invalid("each evidence item has exactly one of flag_id or observation_id".into())),
        }
        out.push(EvidenceRef { flag_id: flag, observation_id: obs });
    }
    Ok(out)
}

/// Create a proposal (status `proposed`). Cited `open` flags of this user
/// become `proposed` and link to it, in the same transaction.
pub fn create_proposal(db: &DbHandle, user_id: &str, new: &NewProposal) -> Result<Proposal> {
    user_required(user_id)?;
    let target_kind = new.target_kind.trim();
    one_of("target_kind", target_kind, TARGET_KINDS)?;
    one_of("origin", new.origin.trim(), ORIGINS)?;
    let target_ref = valid_target_ref(target_kind, &new.target_ref)?;
    let title = required("title", &new.title, MAX_TITLE_CHARS)?;
    if new.body.trim().is_empty() {
        return Err(StoreError::Invalid("body is required".into()));
    }
    if new.body.chars().count() > MAX_BODY_CHARS {
        return Err(StoreError::Invalid(format!("body is longer than {MAX_BODY_CHARS} characters")));
    }
    if new.base.as_deref().is_some_and(|b| b.chars().count() > MAX_BODY_CHARS) {
        return Err(StoreError::Invalid(format!("base is longer than {MAX_BODY_CHARS} characters")));
    }
    let id = format!("kprop_{}", uuid::Uuid::new_v4().simple());
    let mut conn = db.connect()?;
    let tx = conn.transaction()?;
    let evidence = checked_evidence(&tx, user_id, &new.evidence)?;
    let evidence_json = serde_json::to_string(&evidence).map_err(|e| StoreError::Invalid(e.to_string()))?;
    let eval_json = match &new.eval {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.to_string()),
    };
    tx.execute(
        "INSERT INTO knowledge_proposals (id, user_id, target_kind, target_ref, title, body, base, evidence_json,
             origin, eval_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![id, user_id, target_kind, target_ref, title, new.body, new.base, evidence_json, new.origin.trim(), eval_json],
    )?;
    for flag_id in evidence.iter().filter_map(|e| e.flag_id.as_deref()) {
        tx.execute(
            "UPDATE knowledge_flags SET status = 'proposed', proposal_id = ?3, updated_at = datetime('now')
             WHERE user_id = ?1 AND id = ?2 AND status = 'open'",
            params![user_id, flag_id, id],
        )?;
    }
    let out = proposal_on(&tx, user_id, &id)?.ok_or(StoreError::NotFound)?;
    tx.commit()?;
    Ok(out)
}

/// The user's proposals, newest first, optionally of one status. `limit` 1–200.
pub fn list_proposals(db: &DbHandle, user_id: &str, status: Option<&str>, limit: i64) -> Result<Vec<Proposal>> {
    if let Some(s) = status {
        one_of("status", s, PROPOSAL_STATUSES)?;
    }
    let conn = db.connect()?;
    let sql = format!(
        "SELECT {PROPOSAL_COLS} FROM knowledge_proposals WHERE user_id = ?1 AND (?2 IS NULL OR status = ?2)
         ORDER BY created_at DESC, rowid DESC LIMIT ?3"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![user_id, status, limit.clamp(1, 200)], proposal_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get_proposal(db: &DbHandle, user_id: &str, id: &str) -> Result<Option<Proposal>> {
    proposal_on(&db.connect()?, user_id, id)
}

/// Attach or replace the evaluation of a `proposed` proposal (skill growth
/// evaluates after it proposes). `eval` must be a JSON object; once the
/// proposal is decided the evaluation is fixed (409).
pub fn set_proposal_eval(db: &DbHandle, user_id: &str, id: &str, eval: &Value) -> Result<Proposal> {
    user_required(user_id)?;
    if !eval.is_object() {
        return Err(StoreError::Invalid("eval must be an object".into()));
    }
    let eval_json = eval.to_string();
    if eval_json.chars().count() > MAX_BODY_CHARS {
        return Err(StoreError::Invalid(format!("eval is longer than {MAX_BODY_CHARS} characters")));
    }
    let mut conn = db.connect()?;
    let tx = conn.transaction()?;
    let n = tx.execute(
        "UPDATE knowledge_proposals SET eval_json = ?3, updated_at = datetime('now')
         WHERE user_id = ?1 AND id = ?2 AND status = 'proposed'",
        params![user_id, id, eval_json],
    )?;
    if n == 0 {
        return match proposal_on(&tx, user_id, id)? {
            None => Err(StoreError::NotFound),
            Some(p) => Err(StoreError::Conflict(format!("proposal is {}", p.status))),
        };
    }
    let out = proposal_on(&tx, user_id, id)?.ok_or(StoreError::NotFound)?;
    tx.commit()?;
    Ok(out)
}

/// Move a proposal to its next status. The `UPDATE` only matches from the
/// allowed states, so two racing approvals give one 200 and one 409.
/// Approve and reject also resolve the linked flags (same transaction).
pub fn update_proposal_status(db: &DbHandle, user_id: &str, id: &str, decision: Decision) -> Result<Proposal> {
    let mut conn = db.connect()?;
    let tx = conn.transaction()?;
    let from = decision.from_states();
    let placeholders = (0..from.len()).map(|i| format!("?{}", i + 4)).collect::<Vec<_>>().join(", ");
    let decided = if decision.resolves_flags() { ", decided_at = datetime('now')" } else { "" };
    let sql = format!(
        "UPDATE knowledge_proposals SET status = ?3, updated_at = datetime('now'){decided}
         WHERE user_id = ?1 AND id = ?2 AND status IN ({placeholders})"
    );
    let mut args: Vec<String> = vec![user_id.to_string(), id.to_string(), decision.to_state().to_string()];
    args.extend(from.iter().map(|s| s.to_string()));
    let n = tx.execute(&sql, rusqlite::params_from_iter(args.iter()))?;
    if n == 0 {
        return match proposal_on(&tx, user_id, id)? {
            None => Err(StoreError::NotFound),
            Some(p) => Err(StoreError::Conflict(format!("proposal is {}", p.status))),
        };
    }
    if decision.resolves_flags() {
        tx.execute(
            "UPDATE knowledge_flags SET status = 'resolved', updated_at = datetime('now')
             WHERE user_id = ?1 AND proposal_id = ?2 AND status IN ('open', 'proposed')",
            params![user_id, id],
        )?;
    }
    let out = proposal_on(&tx, user_id, id)?.ok_or(StoreError::NotFound)?;
    tx.commit()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_kernel_service as kernel;

    fn flag(kind: &str, target: &str) -> NewFlag {
        NewFlag {
            kind: kind.into(),
            target_kind: target.into(),
            target_ref: Some("github".into()),
            evidence: "Asked how to list open PRs; no guide for the GitHub connection.".into(),
            suggested_fix: Some("Write a data-access guide.".into()),
            session_id: Some("ses_1".into()),
            turn_id: Some("turn_1".into()),
            agent_id: None,
        }
    }

    fn proposal(evidence: Vec<EvidenceRef>) -> NewProposal {
        NewProposal {
            target_kind: "data_access".into(),
            target_ref: "github".into(),
            title: "GitHub data-access guide".into(),
            body: "# GitHub\nOpen PRs: `gh pr list`.".into(),
            base: None,
            evidence,
            origin: "curator".into(),
            eval: Some(serde_json::json!({ "score": 0.8 })),
        }
    }

    fn cite_flag(id: &str) -> EvidenceRef {
        EvidenceRef { flag_id: Some(id.into()), observation_id: None }
    }

    #[test]
    fn knowledge_flag_is_stored_and_returned_as_its_row() {
        let db = DbHandle::new_memory().unwrap();
        let f = create_flag(&db, "u1", &flag("missing", "data_access")).unwrap();
        assert!(f.id.starts_with("kflag_"));
        assert_eq!((f.status.as_str(), f.kind.as_str(), f.target_kind.as_str()), ("open", "missing", "data_access"));
        let (status, session, evidence): (String, Option<String>, String) = db
            .connect()
            .unwrap()
            .query_row("SELECT status, session_id, evidence FROM knowledge_flags WHERE id = ?1", [&f.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((status.as_str(), session.as_deref()), ("open", Some("ses_1")));
        assert!(evidence.contains("GitHub connection"));
        let json = serde_json::to_value(&f).unwrap();
        for col in ["id", "user_id", "agent_id", "session_id", "turn_id", "kind", "target_kind", "target_ref",
            "evidence", "suggested_fix", "status", "proposal_id", "created_at", "updated_at"] {
            assert!(json.get(col).is_some(), "missing {col} in {json}");
        }
        assert_eq!(json["status"], "open");
        assert_eq!(list_flags(&db, "u1", Some("open"), 50).unwrap(), vec![f]);
    }

    #[test]
    fn knowledge_flags_are_scoped_to_their_user() {
        let db = DbHandle::new_memory().unwrap();
        let a = create_flag(&db, "user-a", &flag("wrong", "skill")).unwrap();
        assert!(list_flags(&db, "user-b", None, 50).unwrap().is_empty());
        assert!(get_flag(&db, "user-b", &a.id).unwrap().is_none());
        assert!(matches!(dismiss_flag(&db, "user-b", &a.id), Err(StoreError::NotFound)));
        // User B cannot cite A's flag as evidence either.
        assert!(matches!(create_proposal(&db, "user-b", &proposal(vec![cite_flag(&a.id)])), Err(StoreError::Invalid(_))));
        assert_eq!(get_flag(&db, "user-a", &a.id).unwrap().unwrap().status, "open");
        assert_eq!(list_flags(&db, "user-a", None, 50).unwrap().len(), 1);
    }

    #[test]
    fn knowledge_bad_kind_is_refused_by_the_store_and_the_check_constraint() {
        let db = DbHandle::new_memory().unwrap();
        assert!(matches!(create_flag(&db, "u1", &flag("bogus", "skill")), Err(StoreError::Invalid(_))));
        assert!(matches!(create_flag(&db, "u1", &flag("missing", "website")), Err(StoreError::Invalid(_))));
        let conn = db.connect().unwrap();
        let raw = conn.execute(
            "INSERT INTO knowledge_flags (id, user_id, kind, target_kind, evidence) VALUES ('x', 'u1', 'bogus', 'skill', 'e')",
            [],
        );
        assert!(raw.unwrap_err().to_string().contains("CHECK"), "the CHECK constraint must refuse a bad kind");
        let raw = conn.execute(
            "INSERT INTO knowledge_proposals (id, user_id, target_kind, target_ref, title, body, origin)
             VALUES ('p', 'u1', 'domain', 'r', 't', 'b', 'robot')",
            [],
        );
        assert!(raw.unwrap_err().to_string().contains("CHECK"), "the CHECK constraint must refuse a bad origin");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM knowledge_flags", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn knowledge_approve_resolves_linked_flags() {
        let db = DbHandle::new_memory().unwrap();
        let f1 = create_flag(&db, "u1", &flag("missing", "data_access")).unwrap();
        let f2 = create_flag(&db, "u1", &flag("stale", "data_access")).unwrap();
        let other = create_flag(&db, "u1", &flag("wrong", "skill")).unwrap();
        let obs = kernel::record_observation(&db, "u1", None, Some("ses_1"), "turn_user", "how do I list PRs", Some("user")).unwrap();
        let mut ev = vec![cite_flag(&f1.id), cite_flag(&f2.id)];
        ev.push(EvidenceRef { flag_id: None, observation_id: Some(obs.clone()) });
        let p = create_proposal(&db, "u1", &proposal(ev)).unwrap();
        assert_eq!(p.status, "proposed");
        assert_eq!(p.evidence.len(), 3);
        assert_eq!(p.eval, Some(serde_json::json!({ "score": 0.8 })));
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["evidence"][2]["observation_id"], serde_json::json!(obs));
        assert!(json["evidence"][0].get("observation_id").is_none());
        let f1_now = get_flag(&db, "u1", &f1.id).unwrap().unwrap();
        assert_eq!((f1_now.status.as_str(), f1_now.proposal_id.as_deref()), ("proposed", Some(p.id.as_str())));

        let approved = update_proposal_status(&db, "u1", &p.id, Decision::Approve).unwrap();
        assert_eq!(approved.status, "approved");
        assert!(approved.decided_at.is_some());
        for id in [&f1.id, &f2.id] {
            assert_eq!(get_flag(&db, "u1", id).unwrap().unwrap().status, "resolved");
        }
        assert_eq!(get_flag(&db, "u1", &other.id).unwrap().unwrap().status, "open", "an unlinked flag is untouched");
        // Approving twice is a conflict, not a second change.
        assert!(matches!(update_proposal_status(&db, "u1", &p.id, Decision::Approve), Err(StoreError::Conflict(_))));
        // Another user cannot decide it.
        assert!(matches!(update_proposal_status(&db, "u2", &p.id, Decision::Reject), Err(StoreError::NotFound)));
        assert_eq!(update_proposal_status(&db, "u1", &p.id, Decision::Activated).unwrap().status, "active");
        assert_eq!(update_proposal_status(&db, "u1", &p.id, Decision::Rollback).unwrap().status, "rolled_back");
        assert_eq!(list_proposals(&db, "u1", Some("rolled_back"), 50).unwrap().len(), 1);
        assert!(list_proposals(&db, "u2", None, 50).unwrap().is_empty());
    }

    #[test]
    fn knowledge_reject_resolves_flags_and_dismiss_leaves_the_proposal() {
        let db = DbHandle::new_memory().unwrap();
        let f1 = create_flag(&db, "u1", &flag("missing", "domain")).unwrap();
        let f2 = create_flag(&db, "u1", &flag("missing", "domain")).unwrap();
        let mut np = proposal(vec![cite_flag(&f1.id), cite_flag(&f2.id)]);
        np.target_kind = "domain".into();
        let p = create_proposal(&db, "u1", &np).unwrap();
        assert_eq!(dismiss_flag(&db, "u1", &f2.id).unwrap().status, "dismissed");
        assert_eq!(get_proposal(&db, "u1", &p.id).unwrap().unwrap().status, "proposed");
        update_proposal_status(&db, "u1", &p.id, Decision::Reject).unwrap();
        assert_eq!(get_flag(&db, "u1", &f1.id).unwrap().unwrap().status, "resolved");
        assert_eq!(get_flag(&db, "u1", &f2.id).unwrap().unwrap().status, "dismissed", "a dismissed flag stays dismissed");
        assert!(matches!(dismiss_flag(&db, "u1", &f1.id), Err(StoreError::Conflict(_))));
        // A rejected proposal cannot be activated.
        assert!(matches!(update_proposal_status(&db, "u1", &p.id, Decision::Activated), Err(StoreError::Conflict(_))));
    }

    #[test]
    fn knowledge_proposal_input_is_validated() {
        let db = DbHandle::new_memory().unwrap();
        let mut np = proposal(vec![]);
        np.target_ref = "../../etc/passwd".into();
        assert!(matches!(create_proposal(&db, "u1", &np), Err(StoreError::Invalid(_))));
        let mut np = proposal(vec![]);
        np.origin = "robot".into();
        assert!(matches!(create_proposal(&db, "u1", &np), Err(StoreError::Invalid(_))));
        let both = EvidenceRef { flag_id: Some("a".into()), observation_id: Some("b".into()) };
        assert!(matches!(create_proposal(&db, "u1", &proposal(vec![both])), Err(StoreError::Invalid(_))));
        let unknown_obs = EvidenceRef { flag_id: None, observation_id: Some("obs_nope".into()) };
        assert!(matches!(create_proposal(&db, "u1", &proposal(vec![unknown_obs])), Err(StoreError::Invalid(_))));
        let mut np = proposal(vec![]);
        np.target_kind = "skill".into();
        np.target_ref = "plugin:release-notes".into();
        assert_eq!(create_proposal(&db, "u1", &np).unwrap().target_ref, "plugin:release-notes");
        let n: i64 = db.connect().unwrap().query_row("SELECT COUNT(*) FROM knowledge_proposals", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "refused proposals write nothing");
    }

    #[test]
    fn knowledge_eval_is_set_only_while_proposed() {
        let db = DbHandle::new_memory().unwrap();
        let mut np = proposal(vec![]);
        np.eval = None;
        let p = create_proposal(&db, "u1", &np).unwrap();
        assert_eq!(p.eval, None);
        let eval = serde_json::json!({ "score": 0.9, "report": "passes 9 of 10" });
        assert_eq!(set_proposal_eval(&db, "u1", &p.id, &eval).unwrap().eval, Some(eval.clone()));
        assert!(matches!(set_proposal_eval(&db, "u1", &p.id, &serde_json::json!(0.9)), Err(StoreError::Invalid(_))));
        assert!(matches!(set_proposal_eval(&db, "u2", &p.id, &eval), Err(StoreError::NotFound)));
        update_proposal_status(&db, "u1", &p.id, Decision::Approve).unwrap();
        assert!(matches!(set_proposal_eval(&db, "u1", &p.id, &eval), Err(StoreError::Conflict(_))));
    }
}
