//! The hypothesis queue (D11, step 5.4).
//!
//! Dream lessons and REM insight links are guesses. With the `rem` stage on
//! they are not written as facts or links straight away: each becomes a row in
//! `sleep_hypotheses` with a metric, and is kept only if the metric improves.
//! A guess that does not score a gain is `rejected` and writes nothing.
//!
//! **Gate (pick):** the `rem` stage flag (master switch + `stage.rem`) gates
//! both the lesson queue and REM's links. Its flag text already reads "REM and
//! the hypothesis queue", and one switch keeps the rule simple: with `rem` off
//! Dream writes its lessons exactly as before, whatever else is on. The
//! orchestrator flag is not used, because a Dream also runs from the old clock
//! when the orchestrator is off, and its lessons should follow one rule.
//!
//! **Lesson metric (`evidence_reach`, no model call):** for each evidence turn,
//! the query is that turn's own content words. A turn counts as reached when
//! the keyword index returns, in its top `checks_recall_k`, a fact that was
//! drawn from that turn (`source_observation_id`). `score_before` is the number
//! of evidence turns reached today. `score_after` repeats it with the lesson
//! inserted as a temporary fact inside a SQLite savepoint that is rolled back,
//! counting a turn as reached when the lesson also comes back for it. The
//! lesson is kept only if `score_after > score_before` by at least
//! `hypothesis_keep_margin`.
//!
//! Keyword search only: the metric must not move with embedding noise, so a
//! lesson that shares no words with its evidence can never score a gain.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

use super::config::{self, Resolved};
use super::types::Stage;
use crate::memory_index::{self, Scope};
use crate::memory_relations::{self, NodeKind, RelationType};

pub const KIND_LESSON: &str = "lesson";
pub const KIND_LINK: &str = "link";
pub const METRIC_LESSON: &str = "evidence_reach";
pub const METRIC_LINK: &str = "seed_reach_gain";

/// True when the `rem` stage is on for the user (master switch and flag).
pub fn rem_on(conn: &Connection, user_id: &str) -> bool {
    config::load(conn, user_id).map(|c| c.stage_on(Stage::Rem)).unwrap_or(false)
}

/// Kept only if the metric rose, and by at least the margin.
pub fn margin_met(cfg: &Resolved, before: f64, after: f64) -> bool {
    after > before && after - before >= cfg.number("hypothesis_keep_margin")
}

pub fn enqueue(conn: &Connection, user_id: &str, cycle_id: Option<&str>, kind: &str, content: &str, refs: &Value, metric: &str) -> rusqlite::Result<String> {
    let id = format!("hyp_{}", Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO sleep_hypotheses (id, user_id, cycle_id, kind, content, refs_json, metric, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending')",
        params![id, user_id, cycle_id, kind, content, refs.to_string(), metric],
    )?;
    Ok(id)
}

/// Records the metric's two scores and the decision.
pub fn decide(conn: &Connection, id: &str, before: f64, after: f64, keep: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sleep_hypotheses SET score_before = ?2, score_after = ?3, state = ?4, decided_at = CURRENT_TIMESTAMP WHERE id = ?1",
        params![id, before, after, if keep { "kept" } else { "rejected" }],
    )?;
    Ok(())
}

// ── Shared metric helpers (REM uses them too) ───────────────────────────────

/// Content words of `text`: lowercase, 3+ letters, no stopwords, longest
/// first, at most 12. The query a memory's own words make.
pub fn query_words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && memory_index::fts_query(w).is_some())
        .filter(|w| seen.insert(w.clone()))
        .collect();
    words.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
    words.truncate(12);
    words
}

/// Fact ids the keyword index returns for `query`, best first.
pub fn top_facts(conn: &Connection, user_id: &str, query: &str, limit: usize) -> rusqlite::Result<Vec<String>> {
    let scope = Scope { scope: user_id, target_types: &["fact"], only_ids: None };
    Ok(memory_index::keyword_search(conn, &scope, query, limit)?.into_iter().map(|(_, id)| id).collect())
}

/// Runs `f`, then rolls back every write it made. Nothing it does persists.
pub fn rolled_back<T>(conn: &Connection, f: impl FnOnce() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    conn.execute_batch("SAVEPOINT sleep_hypothesis")?;
    let result = f();
    let undo = conn.execute_batch("ROLLBACK TO sleep_hypothesis; RELEASE sleep_hypothesis");
    let value = result?;
    undo?;
    Ok(value)
}

// ── Dream lessons ───────────────────────────────────────────────────────────

/// A lesson Dream proposed: its text and the (turn id, turn content) it cites.
#[derive(Debug, Clone)]
pub struct LessonCandidate {
    pub entry_id: String,
    pub text: String,
    pub evidence: Vec<(String, String)>,
}

/// (turns reached without the lesson, turns reached with it).
pub fn lesson_scores(conn: &Connection, user_id: &str, k: usize, cand: &LessonCandidate) -> rusqlite::Result<(f64, f64)> {
    let queries: Vec<String> = cand.evidence.iter().map(|(_, content)| query_words(content).join(" ")).collect();
    let mut before = vec![false; cand.evidence.len()];
    for (i, (obs, _)) in cand.evidence.iter().enumerate() {
        for id in top_facts(conn, user_id, &queries[i], k)? {
            let drawn: Option<i64> = conn
                .query_row("SELECT 1 FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND source_observation_id = ?3", params![id, user_id, obs], |r| r.get(0))
                .optional()?;
            if drawn.is_some() {
                before[i] = true;
                break;
            }
        }
    }
    let tmp = format!("tmp_hyp_{}", Uuid::new_v4().simple());
    let after = rolled_back(conn, || {
        conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, ?2, ?3, 0.3)", params![tmp, user_id, cand.text])?;
        let mut n = 0usize;
        for i in 0..cand.evidence.len() {
            if before[i] || top_facts(conn, user_id, &queries[i], k)?.iter().any(|id| *id == tmp) {
                n += 1;
            }
        }
        Ok(n)
    })?;
    Ok((before.iter().filter(|b| **b).count() as f64, after as f64))
}

/// Scores one lesson, records the hypothesis and its decision. Returns the
/// hypothesis id and whether the lesson is kept.
pub fn judge_lesson(conn: &Connection, user_id: &str, cycle_id: Option<&str>, cfg: &Resolved, cand: &LessonCandidate) -> rusqlite::Result<(String, bool)> {
    let k = cfg.number("checks_recall_k").max(1.0) as usize;
    let (before, after) = lesson_scores(conn, user_id, k, cand)?;
    let evidence: Vec<&str> = cand.evidence.iter().map(|(id, _)| id.as_str()).collect();
    let id = enqueue(conn, user_id, cycle_id, KIND_LESSON, &cand.text, &json!({ "entry": cand.entry_id, "evidence": evidence }), METRIC_LESSON)?;
    let keep = !cand.evidence.is_empty() && margin_met(cfg, before, after);
    decide(conn, &id, before, after, keep)?;
    Ok((id, keep))
}

/// After a kept lesson is written as a fact: mark it `dream_generated` with
/// the low credence of its source, and point it back at every evidence turn
/// (`caused_by` edges, origin `dream_lesson`). Idempotent per evidence turn.
pub fn finalize_lesson(conn: &Connection, user_id: &str, hypothesis_id: &str, fact_id: &str, evidence: &[String]) -> rusqlite::Result<()> {
    let cfg = config::load(conn, user_id)?;
    conn.execute(
        "UPDATE memory_facts SET source_type = 'dream_generated', credence = ?3,
                attention = COALESCE(attention, ?4)
         WHERE id = ?1 AND user_id = ?2",
        params![fact_id, user_id, super::score::credence_for(&cfg, super::types::SourceType::DreamGenerated), cfg.number("attention_base")],
    )?;
    for obs in evidence {
        let have: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM memory_relationships WHERE user_id = ?1 AND source_entity_id = ?2 AND target_entity_id = ?3 AND origin = ?4",
                params![user_id, fact_id, obs, memory_relations::ORIGIN_DREAM_LESSON],
                |r| r.get(0),
            )
            .optional()?;
        if have.is_none() {
            memory_relations::write_relation(
                conn, user_id, (NodeKind::Fact, fact_id), RelationType::CausedBy, (NodeKind::Observation, obs),
                0.5, memory_relations::ORIGIN_DREAM_LESSON, Some(hypothesis_id),
            )?;
        }
    }
    let refs: Option<String> = conn
        .query_row("SELECT refs_json FROM sleep_hypotheses WHERE id = ?1", params![hypothesis_id], |r| r.get(0))
        .optional()?
        .flatten();
    let mut v: Value = refs.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
    v["fact"] = json!(fact_id);
    conn.execute("UPDATE sleep_hypotheses SET refs_json = ?2 WHERE id = ?1", params![hypothesis_id, v.to_string()])?;
    Ok(())
}

/// (kept, rejected) hypotheses of one kind decided since `since` (a
/// `sleep_cycles` timestamp).
pub fn decided_since(conn: &Connection, user_id: &str, kind: &str, since: &str) -> rusqlite::Result<(i64, i64)> {
    let n = |state: &str| -> rusqlite::Result<i64> {
        conn.query_row(
            "SELECT COUNT(*) FROM sleep_hypotheses WHERE user_id = ?1 AND kind = ?2 AND state = ?3 AND created_at >= ?4",
            params![user_id, kind, state, since],
            |r| r.get(0),
        )
    };
    Ok((n("kept")?, n("rejected")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use crate::memory_dream::{queue_lessons, Checked, CheckedLesson, Evidence};
    use crate::memory_drive::{Entry, Operation};
    use crate::memory_kernel_service as kernel;

    fn db() -> DbHandle {
        DbHandle::new_memory().unwrap()
    }

    fn on(conn: &Connection, rem: bool) {
        let mut v = vec![("enabled".to_string(), json!(true))];
        if rem {
            v.push(("stage.rem".to_string(), json!(true)));
        }
        config::set_owner(conn, "u1", config::Group::Setting, &v).unwrap();
    }

    fn turn(db: &DbHandle, text: &str) -> Evidence {
        let id = kernel::record_observation(db, "u1", None, None, "turn_user", text, Some("user")).unwrap();
        Evidence { id, session: None, label: None, kind: "turn_user".into(), at: "2026-11-19".into(), content: text.into() }
    }

    fn lesson(text: &str, evidence: &[&Evidence]) -> Checked {
        let mut entry = Entry {
            id: crate::memory_drive_writer::new_entry_id(), text: text.into(), source: "x".into(), added: "2026-11-19".into(), metadata: Default::default(),
        };
        entry.metadata.insert("origin".into(), "dream".into());
        let mut c = Checked::default();
        c.report.push(format!("- **Lesson** added to `lessons.md`: “{text}”"));
        c.lessons.push(CheckedLesson { entry_id: entry.id.clone(), text: text.into(), evidence: evidence.iter().map(|e| e.id.clone()).collect(), report_line: 0 });
        c.ops.push(Operation::UpsertEntry { path: "lessons.md".into(), entry });
        c.summary.lessons = 1;
        c
    }

    fn state_of(conn: &Connection, id: &str) -> String {
        conn.query_row("SELECT state FROM sleep_hypotheses WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn a_lesson_with_no_score_gain_writes_no_fact() {
        let db = db();
        let conn = db.connect().unwrap();
        on(&conn, true);
        let e = turn(&db, "Deploys of the billing service broke because the staging database migration ran late");
        let mut c = lesson("Prefers tea over coffee in the afternoon", &[&e]);
        let kept = queue_lessons(&conn, "u1", &mut c, &[e.clone()]);
        assert!(kept.is_empty());
        assert!(c.ops.is_empty(), "no write for a lesson that scored no gain");
        assert_eq!(c.summary.lessons, 0);
        assert!(c.report[0].contains("not kept"));
        let (state, before, after): (String, f64, f64) = conn
            .query_row("SELECT state, score_before, score_after FROM sleep_hypotheses WHERE kind = 'lesson'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((state.as_str(), before, after), ("rejected", 0.0, 0.0));
        let tmp: i64 = conn.query_row("SELECT COUNT(*) FROM memory_facts WHERE id LIKE 'tmp_hyp_%'", [], |r| r.get(0)).unwrap();
        assert_eq!(tmp, 0, "the temporary fact was rolled back");
    }

    #[test]
    fn a_lesson_that_makes_evidence_reachable_is_kept_and_written_with_back_pointers() {
        let db = db();
        let conn = db.connect().unwrap();
        on(&conn, true);
        let e1 = turn(&db, "The billing service deploy failed because the staging migration ran late");
        let e2 = turn(&db, "Another billing service deploy stalled when the staging migration ran late again");
        let mut c = lesson("Billing service deploys fail when the staging migration runs late", &[&e1, &e2]);
        let kept = queue_lessons(&conn, "u1", &mut c, &[e1.clone(), e2.clone()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(c.ops.len(), 1, "the lesson stays in the ops Dream writes as today");
        assert_eq!(c.summary.lessons, 1);
        let (state, before, after): (String, f64, f64) = conn
            .query_row("SELECT state, score_before, score_after FROM sleep_hypotheses WHERE id = ?1", [&kept[0].hypothesis_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((state.as_str(), before, after), ("kept", 0.0, 2.0));

        // Dream then writes the entry; the fact exists. Finalize it.
        conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES ('f_lesson', 'u1', 'Billing service deploys fail when the staging migration runs late', 0.85)", []).unwrap();
        finalize_lesson(&conn, "u1", &kept[0].hypothesis_id, "f_lesson", &kept[0].evidence).unwrap();
        let (src, cred): (String, f64) = conn.query_row("SELECT source_type, credence FROM memory_facts WHERE id = 'f_lesson'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(src, "dream_generated");
        assert!((cred - 0.3).abs() < 1e-9, "low credence of a dream: {cred}");
        let back: Vec<String> = conn
            .prepare("SELECT target_entity_id FROM memory_relationships WHERE source_entity_id = 'f_lesson' AND origin = 'dream_lesson' ORDER BY target_entity_id")
            .unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        let mut want = vec![e1.id.clone(), e2.id.clone()];
        want.sort();
        assert_eq!(back, want);
        assert_eq!(state_of(&conn, &kept[0].hypothesis_id), "kept");
        // Idempotent.
        finalize_lesson(&conn, "u1", &kept[0].hypothesis_id, "f_lesson", &kept[0].evidence).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM memory_relationships WHERE origin = 'dream_lesson'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn stage_off_dream_lessons_write_facts_exactly_as_today() {
        let db = db();
        let conn = db.connect().unwrap();
        on(&conn, false);
        let e = turn(&db, "Deploys of the billing service broke because the staging database migration ran late");
        let mut c = lesson("Prefers tea over coffee in the afternoon", &[&e]);
        let kept = queue_lessons(&conn, "u1", &mut c, &[e.clone()]);
        assert!(kept.is_empty());
        assert_eq!(c.ops.len(), 1, "the lesson is still written");
        assert_eq!(c.summary.lessons, 1);
        assert!(c.report[0].contains("added to `lessons.md`"));
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM sleep_hypotheses", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "no hypothesis row with the stage off");
    }
}
