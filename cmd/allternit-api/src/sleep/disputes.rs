//! Memory disputes (D10, step 6.1): a contradiction is kept as a record with
//! both sides instead of being silently fixed. Behind the `disputes` stage
//! (master switch + `stage.disputes`); with it off nothing here writes. The
//! flag defaults to on, and the Dream path and `open` need only the flag.
//!
//! **Contradiction or correction.** The extraction model only says `update`
//! (this message changes that memory). [`classify_update`] splits it:
//! - *refinement*: every word of the old fact is still in the new one and no
//!   negation was added ("likes Python" → "likes Python and Rust"). Not a
//!   conflict: it supersedes as before and writes no dispute.
//! - *correction*: the user's own message says the value changed ("I moved",
//!   "no longer", "now", "switched", "actually", ...). The old fact is
//!   superseded as before, and a resolved dispute records it
//!   (`pick = b`, `picked_by = user_said_update`) so the history is kept.
//! - *contradiction*: anything else: a different value with no change word,
//!   or a change that did not come from the user. Both facts stay current and
//!   the [`arbiter`](super::arbiter) decides at once: `resolved` with
//!   `picked_by = arbiter` (the loser is NOT deleted), or `needs_user` with no
//!   pick when the margin is low and the stakes high.
//!
//! Side a is always the older fact, side b the newer one. Dream
//! contradictions open disputes the same way (a = the side Dream wanted to
//! drop, b = the side it kept), and the same pair never gets a second row.
//!
//! The recall monitor ([`notes_for`]) returns the note for each recalled fact
//! that is a side of a dispute still in play: not resolved, or resolved with
//! both sides still current. A note never removes a result.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use super::arbiter::{self, Side};
use super::config::{self, Resolved};
use super::score;
use super::types::{DisputeChoice, DisputeNote, DisputeSide, DisputeView, SourceType, Stage};
use crate::db::DbHandle;

/// `picked_by` values.
pub const BY_ARBITER: &str = "arbiter";
pub const BY_USER: &str = "user";
pub const BY_USER_SAID_UPDATE: &str = "user_said_update";

/// Config for a user when the `disputes` stage is on.
pub fn stage_cfg(conn: &Connection, user_id: &str) -> Option<Resolved> {
    config::load(conn, user_id).ok().filter(|c| c.stage_on(Stage::Disputes))
}

pub fn stage_on(conn: &Connection, user_id: &str) -> bool {
    stage_cfg(conn, user_id).is_some()
}

/// The `disputes` flag alone (default on), without the Sleep master switch.
/// The Dream and the dispute endpoints use it: a contradiction becomes a
/// dispute that keeps both sides, and nothing is removed, for a user who never
/// touched Sleep settings. The owner turns it off with `stage.disputes = false`.
pub fn flag_cfg(conn: &Connection, user_id: &str) -> Option<Resolved> {
    config::load(conn, user_id).ok().filter(|c| c.bool(&config::stage_key(Stage::Disputes)))
}

pub fn flag_on(conn: &Connection, user_id: &str) -> bool {
    flag_cfg(conn, user_id).is_some()
}

// ── Contradiction or correction ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateKind {
    Refinement,
    Correction,
    Contradiction,
}

const CHANGE_CUES: &[&str] = &[
    "moved", "relocated", "now", "nowadays", "these days", "lately", "no longer", "anymore", "any more",
    "switched", "changed", "change", "used to", "instead", "actually", "correction", "i meant", "not anymore",
    "new job", "started", "stopped", "quit", "update", "updated", "promoted", "retired", "married", "divorced",
];

const NEGATIONS: &[&str] = &["not", "no", "never", "don't", "doesn't", "isn't", "aren't", "won't", "nor", "longer", "anymore", "stopped", "quit", "without"];

const STOP: &[&str] = &["the", "and", "for", "are", "was", "with", "has", "have", "that", "this", "its", "their"];

fn raw_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches('\'').to_string())
        .collect()
}

fn content_words(text: &str) -> HashSet<String> {
    raw_words(text).into_iter().filter(|w| w.chars().count() > 2 && !STOP.contains(&w.as_str())).collect()
}

fn negations(text: &str) -> HashSet<String> {
    raw_words(text).into_iter().filter(|w| NEGATIONS.contains(&w.as_str())).collect()
}

/// The message says something changed (word-boundary match).
pub fn has_change_cue(message: &str) -> bool {
    let t = format!(" {} ", raw_words(message).join(" "));
    CHANGE_CUES.iter().any(|c| t.contains(&format!(" {c} ")))
}

/// Classify one extraction `update`. `user_said` = the message is the user's own.
pub fn classify_update(old: &str, new: &str, message: &str, user_said: bool) -> UpdateKind {
    let (o, n) = (content_words(old), content_words(new));
    if !o.is_empty() && o.is_subset(&n) && negations(old) == negations(new) {
        return UpdateKind::Refinement;
    }
    if user_said && has_change_cue(message) {
        return UpdateKind::Correction;
    }
    UpdateKind::Contradiction
}

/// The kind of an extraction update of `old_id` to `new_text` from
/// `observation_id`. `None` when the stage is off or the old fact is gone, so
/// the caller does exactly what it did before.
pub fn plan_update(conn: &Connection, user_id: &str, old_id: &str, new_text: &str, observation_id: &str) -> rusqlite::Result<Option<UpdateKind>> {
    if !stage_on(conn, user_id) {
        return Ok(None);
    }
    let old: Option<String> = conn
        .query_row("SELECT fact FROM memory_facts WHERE id = ?1 AND user_id = ?2", params![old_id, user_id], |r| r.get(0))
        .optional()?;
    let Some(old) = old else { return Ok(None) };
    let obs: Option<(String, String)> = conn
        .query_row(
            "SELECT kind, content FROM memory_observations WHERE id = ?1 AND user_id = ?2",
            params![observation_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (kind, message) = obs.unwrap_or_default();
    let user_said = score::source_for_kind(&kind) == SourceType::UserSaid;
    Ok(Some(classify_update(&old, new_text, &message, user_said)))
}

// ── Sides ───────────────────────────────────────────────────────────────────

struct FactSide {
    text: String,
    credence: f64,
    source_type: Option<String>,
    as_of_jd: Option<f64>,
    agent_id: Option<String>,
}

fn source_type(s: &str) -> Option<SourceType> {
    SourceType::ALL.into_iter().find(|t| t.as_str() == s)
}

/// A current fact as the arbiter sees it. Credence: the step 3.3 column, else
/// the source's configured credence, else the legacy `confidence`.
fn load_side(conn: &Connection, cfg: &Resolved, user_id: &str, id: &str) -> rusqlite::Result<Option<FactSide>> {
    conn.query_row(
        "SELECT fact, credence, source_type, confidence, julianday(valid_from), agent_id FROM memory_facts
         WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
        params![id, user_id],
        |r| {
            let credence: Option<f64> = r.get(1)?;
            let source: Option<String> = r.get(2)?;
            let confidence: f64 = r.get(3)?;
            Ok(FactSide {
                text: r.get(0)?,
                credence: credence
                    .or_else(|| source.as_deref().and_then(source_type).map(|s| score::credence_for(cfg, s)))
                    .unwrap_or(confidence),
                source_type: source,
                as_of_jd: r.get(4)?,
                agent_id: r.get(5)?,
            })
        },
    )
    .optional()
}

fn as_side(f: &FactSide) -> Side<'_> {
    Side { text: &f.text, credence: f.credence, source_type: f.source_type.as_deref(), as_of_jd: f.as_of_jd, agent_id: f.agent_id.as_deref() }
}

fn existing_pair(conn: &Connection, user_id: &str, a: &str, b: &str) -> rusqlite::Result<Option<(String, String)>> {
    conn.query_row(
        "SELECT id, state FROM memory_disputes WHERE user_id = ?1 AND side_a_kind = 'fact' AND side_b_kind = 'fact'
           AND ((side_a_id = ?2 AND side_b_id = ?3) OR (side_a_id = ?3 AND side_b_id = ?2))
         ORDER BY created_at DESC LIMIT 1",
        params![user_id, a, b],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

fn new_id() -> String {
    format!("dsp_{}", uuid::Uuid::new_v4().simple())
}

/// What [`open`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub id: String,
    /// `needs_user` or `resolved`.
    pub state: String,
    pub pick: Option<String>,
    /// False when this pair already had a dispute (nothing was written).
    pub created: bool,
}

/// Open and arbitrate a dispute between two current facts (`a` older, `b`
/// newer). Writes nothing and returns `None` when the stage is off or either
/// fact is no longer current. Neither fact is changed.
pub fn open(conn: &Connection, user_id: &str, a_id: &str, b_id: &str) -> rusqlite::Result<Option<Opened>> {
    let Some(cfg) = flag_cfg(conn, user_id) else { return Ok(None) };
    if a_id == b_id {
        return Ok(None);
    }
    if let Some((id, state)) = existing_pair(conn, user_id, a_id, b_id)? {
        return Ok(Some(Opened { id, state, pick: None, created: false }));
    }
    let (Some(a), Some(b)) = (load_side(conn, &cfg, user_id, a_id)?, load_side(conn, &cfg, user_id, b_id)?) else {
        return Ok(None);
    };
    // No goal store exists yet: the goal input is skipped (see arbiter docs).
    let v = arbiter::decide(&cfg, &as_side(&a), &as_side(&b), None);
    let state = if v.needs_user { "needs_user" } else { "resolved" };
    let pick = v.pick.map(|p| p.as_str().to_string());
    let id = new_id();
    conn.execute(
        "INSERT INTO memory_disputes (id, user_id, side_a_kind, side_a_id, side_b_kind, side_b_id, scope, stakes,
             margin, pick, reason, picked_by, state, resolved_at)
         VALUES (?1, ?2, 'fact', ?3, 'fact', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                 CASE WHEN ?11 = 'resolved' THEN CURRENT_TIMESTAMP END)",
        params![
            id, user_id, a_id, b_id, v.scope, v.stakes, v.margin, pick, v.reason,
            pick.as_ref().map(|_| BY_ARBITER), state
        ],
    )?;
    super::cycle::emit(
        conn,
        user_id,
        super::types::events::DISPUTE_OPENED,
        "Two memories disagree".into(),
        json!({ "dispute": id, "state": state, "scope": v.scope, "stakes": v.stakes, "margin": v.margin }),
        None,
        None,
    );
    if v.needs_user {
        super::cycle::emit(
            conn,
            user_id,
            super::types::events::DISPUTE_NEEDS_USER,
            "A disagreement needs your pick".into(),
            json!({ "dispute": id }),
            None,
            None,
        );
    }
    Ok(Some(Opened { id, state: state.into(), pick, created: true }))
}

/// Record a correction the user made ("I moved to Denver"): a resolved
/// dispute with the newer side picked. The caller supersedes the old fact as
/// it always did. `None` when the stage is off.
pub fn record_correction(conn: &Connection, user_id: &str, old_id: &str, new_id_: &str) -> rusqlite::Result<Option<String>> {
    if !stage_on(conn, user_id) || old_id == new_id_ {
        return Ok(None);
    }
    if let Some((id, _)) = existing_pair(conn, user_id, old_id, new_id_)? {
        return Ok(Some(id));
    }
    let texts: Vec<String> = [old_id, new_id_]
        .iter()
        .filter_map(|id| {
            conn.query_row("SELECT fact FROM memory_facts WHERE id = ?1 AND user_id = ?2", params![id, user_id], |r| r.get(0))
                .optional()
                .ok()
                .flatten()
        })
        .collect();
    let stakes = texts.iter().map(|t| arbiter::stakes_of(t).0).fold(0.0, f64::max);
    let id = new_id();
    conn.execute(
        "INSERT INTO memory_disputes (id, user_id, side_a_kind, side_a_id, side_b_kind, side_b_id, stakes, margin,
             pick, reason, picked_by, state, resolved_at)
         VALUES (?1, ?2, 'fact', ?3, 'fact', ?4, ?5, 1.0, 'b', 'You said this changed.', ?6, 'resolved', CURRENT_TIMESTAMP)",
        params![id, user_id, old_id, new_id_, stakes, BY_USER_SAID_UPDATE],
    )?;
    Ok(Some(id))
}

// ── Reads ───────────────────────────────────────────────────────────────────

const VIEW_COLS: &str = "d.id, d.topic_id, d.side_a_kind, d.side_a_id, d.side_b_kind, d.side_b_id, d.scope, d.stakes,
    d.margin, d.pick, d.reason, d.picked_by, d.state, d.created_at, d.resolved_at,
    fa.fact, fa.id IS NOT NULL AND fa.valid_until IS NULL, fb.fact, fb.id IS NOT NULL AND fb.valid_until IS NULL";

const VIEW_JOIN: &str = "FROM memory_disputes d
    LEFT JOIN memory_facts fa ON d.side_a_kind = 'fact' AND fa.id = d.side_a_id AND fa.user_id = d.user_id
    LEFT JOIN memory_facts fb ON d.side_b_kind = 'fact' AND fb.id = d.side_b_id AND fb.user_id = d.user_id";

fn view_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DisputeView> {
    Ok(DisputeView {
        id: r.get(0)?,
        topic_id: r.get(1)?,
        side_a: DisputeSide { kind: r.get(2)?, id: r.get(3)?, text: r.get(15)?, current: r.get(16)? },
        side_b: DisputeSide { kind: r.get(4)?, id: r.get(5)?, text: r.get(17)?, current: r.get(18)? },
        scope: r.get(6)?,
        stakes: r.get(7)?,
        margin: r.get(8)?,
        pick: r.get(9)?,
        reason: r.get(10)?,
        picked_by: r.get(11)?,
        state: r.get(12)?,
        created_at: r.get(13)?,
        resolved_at: r.get(14)?,
    })
}

/// Disputes for `GET /memory/disputes`, newest first, with both sides' texts.
pub fn list(conn: &Connection, user_id: &str, state: Option<&str>, limit: i64) -> rusqlite::Result<Vec<DisputeView>> {
    let filter = if state.is_some() { "AND d.state = ?2" } else { "" };
    let sql = format!("SELECT {VIEW_COLS} {VIEW_JOIN} WHERE d.user_id = ?1 {filter} ORDER BY d.created_at DESC, d.rowid DESC LIMIT {limit}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = match state {
        Some(s) => stmt.query_map(params![user_id, s], view_row)?.collect::<rusqlite::Result<Vec<_>>>()?,
        None => stmt.query_map(params![user_id], view_row)?.collect::<rusqlite::Result<Vec<_>>>()?,
    };
    Ok(rows)
}

pub fn get(conn: &Connection, user_id: &str, id: &str) -> rusqlite::Result<Option<DisputeView>> {
    conn.query_row(&format!("SELECT {VIEW_COLS} {VIEW_JOIN} WHERE d.user_id = ?1 AND d.id = ?2"), params![user_id, id], view_row)
        .optional()
}

/// The recall monitor: for each fact id that is a side of a dispute still in
/// play (not resolved, or resolved with both sides current), the newest such
/// dispute as a note. Empty when the stage is off.
pub fn notes_for(conn: &Connection, user_id: &str, fact_ids: &[&str]) -> rusqlite::Result<HashMap<String, DisputeNote>> {
    let mut out = HashMap::new();
    if fact_ids.is_empty() || !stage_on(conn, user_id) {
        return Ok(out);
    }
    let sql = format!(
        "SELECT {VIEW_COLS} {VIEW_JOIN}
         WHERE d.user_id = ?1 AND ((d.side_a_kind = 'fact' AND d.side_a_id = ?2) OR (d.side_b_kind = 'fact' AND d.side_b_id = ?2))
         ORDER BY d.created_at DESC, d.rowid DESC LIMIT 20"
    );
    let mut stmt = conn.prepare(&sql)?;
    for id in fact_ids {
        let views = stmt.query_map(params![user_id, id], view_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(v) = views.into_iter().find(|v| v.state != "resolved" || (v.side_a.current && v.side_b.current)) else { continue };
        let this_side = if v.side_a.id == *id { "a" } else { "b" };
        out.insert(
            (*id).to_string(),
            DisputeNote {
                id: v.id,
                needs_user: v.state == "needs_user",
                state: v.state,
                this_side: this_side.into(),
                side_a: v.side_a,
                side_b: v.side_b,
                pick: v.pick,
                reason: v.reason,
                scope: v.scope,
                picked_by: v.picked_by,
            },
        );
    }
    Ok(out)
}

// ── User resolve ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ResolveError {
    NotFound,
    /// You already settled it (`picked_by` user or user_said_update).
    AlreadySettled,
    Db(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound => write!(f, "Dispute not found"),
            ResolveError::AlreadySettled => write!(f, "You already settled this dispute."),
            ResolveError::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl From<rusqlite::Error> for ResolveError {
    fn from(e: rusqlite::Error) -> Self {
        ResolveError::Db(e.to_string())
    }
}

/// Credence added to the side you picked.
pub const USER_PICK_CREDENCE_NUDGE: f64 = 0.1;

/// Retire facts the way the write path does: through the Memory Drive when it
/// is configured, else (and for facts it does not hold) soft supersession.
fn retire(db: &DbHandle, conn: &Connection, user_id: &str, ids: &[String]) -> Result<(), ResolveError> {
    if ids.is_empty() {
        return Ok(());
    }
    let drive = crate::memory_drive_writer::configured_root(conn).ok().flatten().is_some();
    let legacy = if drive {
        match crate::memory_drive_writer::commit_facts(db, user_id, &[], ids, "Settle a memory dispute") {
            Ok(Some(out)) => out.legacy_retire,
            Ok(None) => ids.to_vec(),
            Err(e) => return Err(ResolveError::Db(e.to_string())),
        }
    } else {
        ids.to_vec()
    };
    for id in legacy {
        crate::memory_relations::supersede_fact(conn, user_id, &id)?;
    }
    Ok(())
}

/// `POST /memory/disputes/:id/resolve`: your pick wins. `a`/`b` supersede the
/// other side and nudge the picked side's credence up; `both_scoped` keeps
/// both (the scope says why); `neither` supersedes both.
pub fn resolve_by_user(db: &DbHandle, user_id: &str, id: &str, choice: DisputeChoice, reason: Option<&str>) -> Result<DisputeView, ResolveError> {
    let conn = db.connect()?;
    let d = get(&conn, user_id, id)?.ok_or(ResolveError::NotFound)?;
    if matches!(d.picked_by.as_deref(), Some(BY_USER) | Some(BY_USER_SAID_UPDATE)) {
        return Err(ResolveError::AlreadySettled);
    }
    let reason = reason.map(str::trim).filter(|r| !r.is_empty()).map(|r| r.chars().take(240).collect::<String>());
    let facts = |s: &DisputeSide| (s.kind == "fact").then(|| s.id.clone());
    let (a, b) = (facts(&d.side_a), facts(&d.side_b));
    let (keep, drop): (Option<String>, Vec<String>) = match choice {
        DisputeChoice::A => (a, b.into_iter().collect()),
        DisputeChoice::B => (b, a.into_iter().collect()),
        DisputeChoice::BothScoped => (None, vec![]),
        DisputeChoice::Neither => (None, a.into_iter().chain(b).collect()),
    };
    let default_reason = match choice {
        DisputeChoice::A | DisputeChoice::B => "You picked this one.",
        DisputeChoice::BothScoped => "You said both are true in different situations.",
        DisputeChoice::Neither => "You said neither is true.",
    };
    let scope = match choice {
        DisputeChoice::BothScoped => Some(d.scope.clone().or_else(|| reason.clone()).unwrap_or_else(|| "both true in different situations".into())),
        _ => d.scope.clone(),
    };
    retire(db, &conn, user_id, &drop)?;
    if let Some(k) = keep {
        conn.execute(
            "UPDATE memory_facts SET credence = MIN(1.0, COALESCE(credence, confidence, 0.5) + ?3) WHERE id = ?1 AND user_id = ?2",
            params![k, user_id, USER_PICK_CREDENCE_NUDGE],
        )?;
    }
    conn.execute(
        "UPDATE memory_disputes SET pick = ?3, picked_by = ?4, reason = ?5, scope = ?6, state = 'resolved',
             margin = 1.0, updated_at = CURRENT_TIMESTAMP, resolved_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND user_id = ?2",
        params![id, user_id, choice.as_str(), BY_USER, reason.as_deref().unwrap_or(default_reason), scope],
    )?;
    get(&conn, user_id, id)?.ok_or(ResolveError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_extraction::{self as ex, MemoryOp};
    use crate::memory_kernel_service as kernel;

    fn on(db: &DbHandle) {
        let conn = db.connect().unwrap();
        for k in ["enabled", "stage.disputes"] {
            conn.execute("INSERT INTO sleep_config (user_id, key, value_json, source) VALUES ('u1', ?1, 'true', 'owner')", [k]).unwrap();
        }
    }

    fn current(db: &DbHandle) -> Vec<String> {
        let mut f: Vec<String> = kernel::list_facts(db, "u1", None, 50).unwrap().into_iter().map(|f| f.fact).collect();
        f.sort();
        f
    }

    fn count(db: &DbHandle) -> i64 {
        db.connect().unwrap().query_row("SELECT COUNT(*) FROM memory_disputes WHERE user_id = 'u1'", [], |r| r.get(0)).unwrap()
    }

    /// One existing fact, then an extraction `update` of it from `message`.
    fn update(db: &DbHandle, old: &str, new: &str, message: &str) -> (String, String) {
        let obs0 = kernel::record_observation(db, "u1", None, None, "turn_user", old, Some("user")).unwrap();
        let old_id = kernel::persist_facts(db, "u1", None, &obs0, &[old.into()]).unwrap().remove(0).id;
        // The old fact is from two months ago.
        db.connect().unwrap().execute("UPDATE memory_facts SET valid_from = datetime('now', '-60 days') WHERE id = ?1", [&old_id]).unwrap();
        let obs = kernel::record_observation(db, "u1", None, None, "turn_user", message, Some("user")).unwrap();
        let shown = ex::candidate_facts(db, "u1", message).unwrap();
        let ops = vec![MemoryOp::Update { id: old_id.clone(), fact: new.into(), memory_type: None }];
        ex::apply_ops(db, "u1", None, &obs, &ops, &shown).unwrap();
        let new_id: String = db.connect().unwrap()
            .query_row("SELECT id FROM memory_facts WHERE user_id = 'u1' AND fact = ?1", [new], |r| r.get(0)).unwrap();
        (old_id, new_id)
    }

    #[test]
    fn contradiction_vs_correction_rule() {
        assert_eq!(classify_update("User likes Python.", "User likes Python and Rust.", "also Rust", true), UpdateKind::Refinement);
        assert_eq!(classify_update("User lives in Austin.", "User no longer lives in Austin.", "nope", true), UpdateKind::Contradiction);
        assert_eq!(classify_update("User lives in Austin.", "User lives in Denver.", "I moved to Denver.", true), UpdateKind::Correction);
        assert_eq!(classify_update("User lives in Austin.", "User lives in Denver.", "I moved to Denver.", false), UpdateKind::Contradiction);
        assert_eq!(classify_update("User lives in Austin.", "User lives in Denver.", "I live in Denver.", true), UpdateKind::Contradiction);
        assert!(!has_change_cue("I know Denver well") && has_change_cue("I'm now in Denver"));
    }

    #[test]
    fn two_contradicting_facts_survive_with_one_dispute_row() {
        let db = DbHandle::new_memory().unwrap();
        on(&db);
        let (old, new) = update(&db, "User lives in Austin.", "User lives in Denver.", "I live in Denver.");
        assert_eq!(current(&db), vec!["User lives in Austin.".to_string(), "User lives in Denver.".to_string()]);
        let conn = db.connect().unwrap();
        let all = list(&conn, "u1", None, 50).unwrap();
        assert_eq!(all.len(), 1);
        let d = &all[0];
        assert_eq!((d.side_a.id.as_str(), d.side_b.id.as_str()), (old.as_str(), new.as_str()));
        assert_eq!((d.side_a.text.as_deref(), d.side_b.text.as_deref()), (Some("User lives in Austin."), Some("User lives in Denver.")));
        assert!(d.side_a.current && d.side_b.current, "the loser is not deleted");
        // Same credence, the newer one wins on recency.
        assert_eq!((d.state.as_str(), d.pick.as_deref(), d.picked_by.as_deref()), ("resolved", Some("b"), Some(BY_ARBITER)));
        assert_eq!(d.reason.as_deref(), Some("It is the more recent of the two."));
        // The same pair again writes no second row.
        assert!(!open(&conn, "u1", &old, &new).unwrap().unwrap().created);
        assert_eq!(count(&db), 1);
    }

    #[test]
    fn a_user_correction_supersedes_and_keeps_a_resolved_record() {
        let db = DbHandle::new_memory().unwrap();
        on(&db);
        update(&db, "User lives in Austin.", "User lives in Denver.", "I moved to Denver last week.");
        assert_eq!(current(&db), vec!["User lives in Denver.".to_string()]);
        let d = list(&db.connect().unwrap(), "u1", None, 50).unwrap().remove(0);
        assert_eq!((d.state.as_str(), d.pick.as_deref(), d.picked_by.as_deref()), ("resolved", Some("b"), Some(BY_USER_SAID_UPDATE)));
        assert!(!d.side_a.current && d.side_b.current);
    }

    #[test]
    fn stage_off_an_update_supersedes_exactly_as_today_and_no_dispute_row_is_written() {
        let db = DbHandle::new_memory().unwrap();
        // The master switch alone does not turn the update path's disputes on
        // once the owner has switched the flag off.
        for (k, v) in [("enabled", "true"), ("stage.disputes", "false")] {
            db.connect().unwrap().execute("INSERT INTO sleep_config (user_id, key, value_json, source) VALUES ('u1', ?1, ?2, 'owner')", [k, v]).unwrap();
        }
        let (old, new) = update(&db, "User lives in Austin.", "User lives in Denver.", "I live in Denver.");
        assert_eq!(current(&db), vec!["User lives in Denver.".to_string()]);
        assert_eq!(count(&db), 0);
        let rel: String = db.connect().unwrap().query_row(
            "SELECT relation FROM memory_relationships WHERE source_entity_id = ?1 AND target_entity_id = ?2",
            [&new, &old], |r| r.get(0)).unwrap();
        assert_eq!(rel, "updates");
    }

    #[test]
    fn opening_a_needs_user_dispute_writes_opened_and_needs_user() {
        let db = DbHandle::new_memory().unwrap();
        on(&db);
        let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let a = kernel::persist_facts(&db, "u1", None, &obs, &["User is allergic to penicillin.".into()]).unwrap().remove(0).id;
        let b = kernel::persist_facts(&db, "u1", None, &obs, &["User is not allergic to penicillin.".into()]).unwrap().remove(0).id;
        let conn = db.connect().unwrap();
        conn.execute("UPDATE memory_facts SET valid_from = '2026-11-01 10:00:00' WHERE id IN (?1, ?2)", [&a, &b]).unwrap();
        let o = open(&conn, "u1", &a, &b).unwrap().unwrap();
        assert_eq!(o.state, "needs_user");
        let kinds = |c: &Connection| -> Vec<String> {
            crate::memory_activity::list(c, "u1", &crate::memory_activity::Filter { after: Some(0), ..Default::default() }).unwrap().into_iter().map(|r| r.kind).collect()
        };
        assert_eq!(kinds(&conn), vec!["sleep.dispute.opened", "sleep.dispute.needs_user"]);
        // Opening the same pair again writes no dispute and no events.
        assert!(!open(&conn, "u1", &a, &b).unwrap().unwrap().created);
        assert_eq!(kinds(&conn).len(), 2);
    }

    #[test]
    fn a_low_margin_high_stakes_case_returns_needs_user_with_no_pick_applied() {
        let db = DbHandle::new_memory().unwrap();
        on(&db);
        let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let a = kernel::persist_facts(&db, "u1", None, &obs, &["User is allergic to penicillin.".into()]).unwrap().remove(0).id;
        let b = kernel::persist_facts(&db, "u1", None, &obs, &["User is not allergic to penicillin.".into()]).unwrap().remove(0).id;
        let conn = db.connect().unwrap();
        conn.execute("UPDATE memory_facts SET valid_from = '2026-11-01 10:00:00' WHERE id IN (?1, ?2)", [&a, &b]).unwrap();
        let o = open(&conn, "u1", &a, &b).unwrap().unwrap();
        assert_eq!((o.state.as_str(), o.pick.clone()), ("needs_user", None));
        let d = get(&conn, "u1", &o.id).unwrap().unwrap();
        assert_eq!((&d.pick, &d.picked_by, &d.resolved_at), (&None, &None, &None));
        assert!(d.stakes.unwrap() >= 0.7 && d.margin.unwrap() < 0.15, "{d:?}");
        // No pick applied: both facts are still current, credence untouched.
        assert_eq!(current(&db).len(), 2);
    }

    #[test]
    fn resolve_with_choice_a_supersedes_b_and_records_picked_by_user() {
        let db = DbHandle::new_memory().unwrap();
        on(&db);
        let (old, new) = update(&db, "User lives in Austin.", "User lives in Denver.", "I live in Denver.");
        let conn = db.connect().unwrap();
        let id = list(&conn, "u1", None, 1).unwrap().remove(0).id;
        let before: f64 = conn.query_row("SELECT credence FROM memory_facts WHERE id = ?1", [&old], |r| r.get(0)).unwrap();
        let d = resolve_by_user(&db, "u1", &id, DisputeChoice::A, Some("I still live in Austin")).unwrap();
        assert_eq!((d.pick.as_deref(), d.picked_by.as_deref(), d.state.as_str()), (Some("a"), Some(BY_USER), "resolved"));
        assert_eq!(d.reason.as_deref(), Some("I still live in Austin"));
        assert!(d.side_a.current && !d.side_b.current);
        assert_eq!(current(&db), vec!["User lives in Austin.".to_string()]);
        let after: f64 = conn.query_row("SELECT credence FROM memory_facts WHERE id = ?1", [&old], |r| r.get(0)).unwrap();
        assert!(after > before);
        let _ = new;
        // A second pick is refused; an unknown id is not found.
        assert!(matches!(resolve_by_user(&db, "u1", &id, DisputeChoice::B, None), Err(ResolveError::AlreadySettled)));
        assert!(matches!(resolve_by_user(&db, "u1", "dsp_nope", DisputeChoice::A, None), Err(ResolveError::NotFound)));
    }

    /// Production writes go through the Memory Drive: the same rule holds there.
    #[test]
    fn on_the_memory_drive_a_contradiction_keeps_both_and_a_user_pick_retires_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let db = DbHandle::new(base.join("test.db")).unwrap();
        crate::memory_drive_writer::configure_root(&db, &base.join("brains")).unwrap();
        on(&db);
        let obs0 = kernel::record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let old = kernel::persist_facts(&db, "u1", None, &obs0, &["User lives in Austin.".into()]).unwrap().remove(0).id;
        let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "I live in Denver.", Some("user")).unwrap();
        let shown = ex::candidate_facts(&db, "u1", "I live in Denver.").unwrap();
        let ops = vec![MemoryOp::Update { id: old.clone(), fact: "User lives in Denver.".into(), memory_type: None }];
        ex::apply_ops(&db, "u1", None, &obs, &ops, &shown).unwrap();
        assert_eq!(current(&db), vec!["User lives in Austin.".to_string(), "User lives in Denver.".to_string()]);
        let id = list(&db.connect().unwrap(), "u1", None, 5).unwrap().remove(0).id;
        let d = resolve_by_user(&db, "u1", &id, DisputeChoice::B, None).unwrap();
        assert!(!d.side_a.current && d.side_b.current, "{d:?}");
        assert_eq!(current(&db), vec!["User lives in Denver.".to_string()]);
    }

    #[test]
    fn both_scoped_keeps_both_and_neither_supersedes_both() {
        let db = DbHandle::new_memory().unwrap();
        on(&db);
        let obs = kernel::record_observation(&db, "u1", None, None, "turn_user", "x", Some("user")).unwrap();
        let mk = |t: &str| kernel::persist_facts(&db, "u1", None, &obs, &[t.into()]).unwrap().remove(0).id;
        let (a, b, c, e) = (mk("User drinks coffee."), mk("User drinks tea."), mk("User runs daily."), mk("User never runs."));
        let conn = db.connect().unwrap();
        let d1 = open(&conn, "u1", &a, &b).unwrap().unwrap().id;
        let d2 = open(&conn, "u1", &c, &e).unwrap().unwrap().id;
        let s = resolve_by_user(&db, "u1", &d1, DisputeChoice::BothScoped, Some("coffee at work, tea at home")).unwrap();
        assert_eq!((s.pick.as_deref(), s.scope.as_deref()), (Some("both_scoped"), Some("coffee at work, tea at home")));
        resolve_by_user(&db, "u1", &d2, DisputeChoice::Neither, None).unwrap();
        assert_eq!(current(&db), vec!["User drinks coffee.".to_string(), "User drinks tea.".to_string()]);
    }
}
