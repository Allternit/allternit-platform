//! REM (D11, step 5.4): insight links through the hypothesis queue.
//!
//! REM is a hypothesis generator, not a source of facts. Behind the `rem`
//! stage it proposes links between memories that already exist, scores each
//! one with a metric, and keeps it only if the metric improves. Candidate
//! generation is deterministic (no model call in this step).
//!
//! **Seeds** (what REM tries to fix):
//! - failed recalls: `memory_recall_logs.label_used = '[]'` (the reply used
//!   nothing it was shown; written by the step 3.1 labels);
//! - corrections: disputes resolved with `picked_by = 'user_said_update'`
//!   (the seed words are the newer fact's);
//! - open disputes (`open` / `needs_user`; the seed words are side a's).
//!
//! **Candidates:** the keyword index is searched with the seed's content
//! words. Hits inside the top `checks_recall_k` are anchors; hits ranked below
//! it, not yet linked to the anchor, are the far ends. A candidate is
//! `anchor -> far end`, a typed `about_entity` edge with origin `rem`.
//!
//! **Link metric (`seed_reach_gain`):** the seed's query is run with neighbour
//! expansion (the top-k hits plus the memories linked to them). The score is
//! how many memories in that set are useful: they share at least
//! `min(2, words)` of the seed's words. `score_before` is that count today,
//! `score_after` repeats it with the edge inserted inside a savepoint that is
//! rolled back. Kept only on a rise of at least `hypothesis_keep_margin`;
//! then the relation is written for real.
//!
//! **An insight is a link that later gets used.** [`note_link_used`] raises
//! `insight_score` and `attention` on both ends when a kept link's endpoint is
//! labelled used. Step 3.1's label writer must call it for each used fact id.
//!
//! **Self-off:** when the keep rate over the last `rem_window_cycles` cycles
//! that decided a link falls under `rem_min_keep_rate`, REM writes a `fitted`
//! `sleep_config` row `rem_auto_off = 1` and skips itself. An `owner` row for
//! the key wins (the user turned it back on).

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::config::{self, Resolved};
use super::hypotheses::{self, KIND_LINK, METRIC_LINK};
use super::types::Stage;
use crate::memory_relations::{self, NodeKind, RelationType, ORIGIN_REM};

pub const KEY_AUTO_OFF: &str = "rem_auto_off";

#[derive(Debug)]
pub enum Outcome {
    /// The `rem` stage is off.
    Off,
    /// REM switched itself off (keep rate) and stays off.
    AutoOff,
    Ran(Value),
}

struct Seed {
    id: String,
    kind: &'static str,
    query: String,
}

fn seeds(conn: &Connection, user_id: &str, limit: usize) -> rusqlite::Result<Vec<Seed>> {
    let fresh = "NOT EXISTS (SELECT 1 FROM sleep_hypotheses h WHERE h.user_id = ?1 AND h.kind = 'link'
                  AND h.refs_json LIKE '%\"seed\":\"' || {id} || '\"%')";
    let mut out = Vec::new();
    let mut run = |sql: String, kind: &'static str| -> rusqlite::Result<()> {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![user_id, limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, query) = row?;
            out.push(Seed { id, kind, query });
        }
        Ok(())
    };
    run(
        format!(
            "SELECT l.id, l.query FROM memory_recall_logs l
             WHERE l.user_id = ?1 AND l.label_used = '[]' AND l.query IS NOT NULL AND {}
             ORDER BY l.created_at DESC, l.id DESC LIMIT ?2",
            fresh.replace("{id}", "l.id")
        ),
        "failed_recall",
    )?;
    run(
        format!(
            "SELECT d.id, f.fact FROM memory_disputes d JOIN memory_facts f ON f.id = d.side_b_id AND f.user_id = d.user_id
             WHERE d.user_id = ?1 AND d.state = 'resolved' AND d.picked_by = 'user_said_update' AND {}
             ORDER BY d.updated_at DESC, d.id DESC LIMIT ?2",
            fresh.replace("{id}", "d.id")
        ),
        "correction",
    )?;
    run(
        format!(
            "SELECT d.id, f.fact FROM memory_disputes d JOIN memory_facts f ON f.id = d.side_a_id AND f.user_id = d.user_id
             WHERE d.user_id = ?1 AND d.state IN ('open', 'needs_user') AND {}
             ORDER BY d.updated_at DESC, d.id DESC LIMIT ?2",
            fresh.replace("{id}", "d.id")
        ),
        "open_dispute",
    )?;
    Ok(out)
}

fn linked(conn: &Connection, user_id: &str, a: &str, b: &str) -> rusqlite::Result<bool> {
    Ok(memory_relations::relations_of(conn, user_id, a)?.iter().any(|(_, s, t)| s == b || t == b))
}

/// How many of the seed's words a fact contains.
fn overlap(conn: &Connection, user_id: &str, words: &[String], id: &str) -> rusqlite::Result<usize> {
    let text: Option<String> = conn
        .query_row("SELECT fact FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL", params![id, user_id], |r| r.get(0))
        .optional()?;
    let Some(text) = text else { return Ok(0) };
    let have: HashSet<String> = text.split(|c: char| !c.is_alphanumeric()).map(str::to_lowercase).collect();
    Ok(words.iter().filter(|w| have.contains(*w)).count())
}

/// Useful memories in the seed's recall with one hop of neighbour expansion.
fn reach_score(conn: &Connection, user_id: &str, query: &str, words: &[String], k: usize) -> rusqlite::Result<f64> {
    let hits = hypotheses::top_facts(conn, user_id, query, k)?;
    let mut set: Vec<String> = hits.clone();
    for h in &hits {
        for (_, s, t) in memory_relations::relations_of(conn, user_id, h)? {
            set.push(if s == *h { t } else { s });
        }
    }
    let need = words.len().min(2).max(1);
    let mut seen = HashSet::new();
    let mut n = 0.0;
    for id in set {
        if seen.insert(id.clone()) && overlap(conn, user_id, words, &id)? >= need {
            n += 1.0;
        }
    }
    Ok(n)
}

/// (before, after) for adding the edge `a -> b`; the edge is rolled back.
pub fn link_scores(conn: &Connection, user_id: &str, query: &str, k: usize, a: &str, b: &str) -> rusqlite::Result<(f64, f64)> {
    let words = hypotheses::query_words(query);
    let q = words.join(" ");
    let before = reach_score(conn, user_id, &q, &words, k)?;
    let after = hypotheses::rolled_back(conn, || {
        memory_relations::write_relation(conn, user_id, (NodeKind::Fact, a), RelationType::AboutEntity, (NodeKind::Fact, b), 0.5, ORIGIN_REM, None)?;
        reach_score(conn, user_id, &q, &words, k)
    })?;
    Ok((before, after))
}

/// Keep rate over the last `rem_window_cycles` cycles that decided a link.
/// None until that many cycles have decided one.
pub fn keep_rate(conn: &Connection, user_id: &str, window: usize) -> rusqlite::Result<Option<f64>> {
    let mut stmt = conn.prepare(
        "SELECT cycle_id FROM sleep_hypotheses
         WHERE user_id = ?1 AND kind = 'link' AND state IN ('kept', 'rejected') AND cycle_id IS NOT NULL
         GROUP BY cycle_id ORDER BY MAX(decided_at) DESC, cycle_id DESC LIMIT ?2",
    )?;
    let cycles: Vec<String> = stmt.query_map(params![user_id, window as i64], |r| r.get(0))?.collect::<Result<_, _>>()?;
    if cycles.len() < window {
        return Ok(None);
    }
    let (mut kept, mut total) = (0i64, 0i64);
    for c in &cycles {
        let (k, t): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(state = 'kept'), 0), COUNT(*) FROM sleep_hypotheses
             WHERE user_id = ?1 AND kind = 'link' AND cycle_id = ?2 AND state IN ('kept', 'rejected')",
            params![user_id, c],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        kept += k;
        total += t;
    }
    Ok((total > 0).then(|| kept as f64 / total as f64))
}

/// Switches REM off when its keep rate is under the floor. True when REM is
/// now off. A user's own `rem_auto_off` row overrides it.
fn check_self_off(conn: &Connection, user_id: &str, cfg: &Resolved) -> rusqlite::Result<bool> {
    let window = cfg.number("rem_window_cycles").max(1.0) as usize;
    let Some(rate) = keep_rate(conn, user_id, window)? else { return Ok(false) };
    if rate >= cfg.number("rem_min_keep_rate") {
        return Ok(false);
    }
    let written = config::set_fitted(conn, user_id, &[(KEY_AUTO_OFF, 1.0)]).map_err(|e| match e {
        config::ConfigError::Db(d) => d,
        other => rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(other.to_string()))),
    })?;
    Ok(!written.is_empty())
}

/// One REM pass inside a nap.
pub fn run_stage(conn: &Connection, user_id: &str, cycle_id: &str, _now: chrono::DateTime<chrono::Utc>) -> rusqlite::Result<Outcome> {
    let cfg = config::load(conn, user_id)?;
    if !cfg.stage_on(Stage::Rem) {
        return Ok(Outcome::Off);
    }
    if cfg.number(KEY_AUTO_OFF) >= 0.5 || check_self_off(conn, user_id, &cfg)? {
        return Ok(Outcome::AutoOff);
    }
    let k = cfg.number("checks_recall_k").max(1.0) as usize;
    let max = cfg.number("rem_max_candidates").max(1.0) as usize;
    let (mut proposed, mut kept, mut rejected) = (0usize, 0usize, 0usize);
    let mut seed_count = 0usize;
    'seeds: for seed in seeds(conn, user_id, max as usize)? {
        seed_count += 1;
        let words = hypotheses::query_words(&seed.query);
        if words.is_empty() {
            continue;
        }
        let q = words.join(" ");
        let pool = hypotheses::top_facts(conn, user_id, &q, k * 3)?;
        let Some(anchor) = pool.iter().take(k).next().cloned() else { continue };
        let mut per_seed = 0;
        for far in pool.iter().skip(k) {
            if proposed >= max {
                break 'seeds;
            }
            if per_seed >= 2 {
                break;
            }
            if *far == anchor || linked(conn, user_id, &anchor, far)? {
                continue;
            }
            per_seed += 1;
            proposed += 1;
            let (before, after) = link_scores(conn, user_id, &seed.query, k, &anchor, far)?;
            let content: String = conn
                .query_row("SELECT fact FROM memory_facts WHERE id = ?1", params![far], |r| r.get(0))
                .unwrap_or_default();
            let refs = json!({ "seed": seed.id, "seed_kind": seed.kind, "a": anchor, "b": far });
            let id = hypotheses::enqueue(conn, user_id, Some(cycle_id), KIND_LINK, &content, &refs, METRIC_LINK)?;
            let keep = hypotheses::margin_met(&cfg, before, after);
            hypotheses::decide(conn, &id, before, after, keep)?;
            if keep {
                let rel = memory_relations::write_relation(conn, user_id, (NodeKind::Fact, &anchor), RelationType::AboutEntity, (NodeKind::Fact, far), 0.5, ORIGIN_REM, Some(&id))?;
                let mut r = refs.clone();
                r["relation"] = json!(rel);
                conn.execute("UPDATE sleep_hypotheses SET refs_json = ?2 WHERE id = ?1", params![id, r.to_string()])?;
                kept += 1;
            } else {
                rejected += 1;
            }
        }
    }
    let since: Option<String> = conn
        .query_row("SELECT started_at FROM sleep_cycles WHERE id = ?1", params![cycle_id], |r| r.get(0))
        .optional()?;
    let (lessons_kept, lessons_rejected) = match since {
        Some(s) => hypotheses::decided_since(conn, user_id, hypotheses::KIND_LESSON, &s)?,
        None => (0, 0),
    };
    let window = cfg.number("rem_window_cycles").max(1.0) as usize;
    let rate = keep_rate(conn, user_id, window)?;
    let auto_off = check_self_off(conn, user_id, &cfg)?;
    Ok(Outcome::Ran(json!({
        "seeds": seed_count, "links_proposed": proposed, "links_kept": kept, "links_rejected": rejected,
        "lessons_kept": lessons_kept, "lessons_rejected": lessons_rejected, "keep_rate": rate, "auto_off": auto_off,
    })))
}

/// A kept REM link's endpoint was labelled used: raise insight and attention
/// on both ends of every kept link it sits on. Returns how many memories were
/// raised. No-op with the `rem` stage off.
pub fn note_link_used(conn: &Connection, user_id: &str, fact_id: &str) -> rusqlite::Result<usize> {
    let cfg = config::load(conn, user_id)?;
    if !cfg.stage_on(Stage::Rem) {
        return Ok(0);
    }
    let step = cfg.number("rem_insight_step");
    let mut stmt = conn.prepare(
        "SELECT source_entity_id, target_entity_id FROM memory_relationships
         WHERE user_id = ?1 AND origin = ?3 AND valid_until IS NULL AND source_kind = 'fact' AND target_kind = 'fact'
           AND (source_entity_id = ?2 OR target_entity_id = ?2)",
    )?;
    let ends: Vec<(String, String)> = stmt.query_map(params![user_id, fact_id, ORIGIN_REM], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut ids = HashSet::new();
    for (s, t) in ends {
        ids.insert(s);
        ids.insert(t);
    }
    for id in &ids {
        conn.execute(
            "UPDATE memory_facts SET insight_score = MIN(1.0, COALESCE(insight_score, 0) + ?3),
                    attention = CASE WHEN attention IS NULL THEN NULL ELSE MIN(1.0, attention + ?3) END
             WHERE id = ?1 AND user_id = ?2",
            params![id, user_id, step],
        )?;
    }
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    fn setup() -> (DbHandle, Connection) {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        config::set_owner(&conn, "u1", config::Group::Setting, &[("enabled".to_string(), json!(true)), ("stage.rem".to_string(), json!(true))]).unwrap();
        config::set_owner(&conn, "u1", config::Group::Threshold, &[("checks_recall_k".to_string(), json!(1))]).unwrap();
        (db, conn)
    }

    fn fact(conn: &Connection, id: &str, text: &str) {
        conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence, attention) VALUES (?1, 'u1', ?2, 0.8, 0.1)", params![id, text]).unwrap();
    }

    fn failed_recall(conn: &Connection) {
        conn.execute(
            "INSERT INTO memory_recall_logs (id, user_id, query, results, created_at, label_used, label_source, labeled_at)
             VALUES ('r1', 'u1', 'how do we deploy billing service', '[]', '2026-11-19 10:00:00', '[]', 'answer', '2026-11-19 10:00:01')",
            [],
        ).unwrap();
    }

    fn run(conn: &Connection, cycle: &str) -> Outcome {
        run_stage(conn, "u1", cycle, chrono::Utc::now()).unwrap()
    }

    #[test]
    fn a_kept_link_raises_both_memories_attention_when_later_used() {
        let (_db, conn) = setup();
        fact(&conn, "fa", "We deploy the billing service every Friday");
        fact(&conn, "fb", "Billing service runbook lives in the ops wiki");
        failed_recall(&conn);
        let Outcome::Ran(d) = run(&conn, "c1") else { panic!("REM did not run") };
        assert_eq!((d["links_proposed"].as_i64(), d["links_kept"].as_i64()), (Some(1), Some(1)), "{d}");
        let (state, before, after): (String, f64, f64) = conn
            .query_row("SELECT state, score_before, score_after FROM sleep_hypotheses WHERE kind = 'link'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((state.as_str(), before, after), ("kept", 1.0, 2.0));
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM memory_relationships WHERE origin = 'rem'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "one real link; the scoring edge was rolled back");

        let att = |id: &str| -> (f64, Option<f64>) {
            conn.query_row("SELECT attention, insight_score FROM memory_facts WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        };
        assert_eq!(att("fa"), (0.1, None));
        assert_eq!(note_link_used(&conn, "u1", "fb").unwrap(), 2);
        for id in ["fa", "fb"] {
            let (a, i) = att(id);
            assert!((a - 0.2).abs() < 1e-9, "{id} attention {a}");
            assert!((i.unwrap() - 0.1).abs() < 1e-9, "{id} insight");
        }
        // The same seed is not tried twice.
        let Outcome::Ran(d) = run(&conn, "c2") else { panic!() };
        assert_eq!(d["links_proposed"].as_i64(), Some(0));
    }

    #[test]
    fn rem_turns_itself_off_under_the_keep_rate() {
        let (_db, conn) = setup();
        config::set_owner(&conn, "u1", config::Group::Threshold, &[("rem_window_cycles".to_string(), json!(3))]).unwrap();
        for c in ["c1", "c2", "c3"] {
            conn.execute(
                "INSERT INTO sleep_hypotheses (id, user_id, cycle_id, kind, content, state, decided_at)
                 VALUES (?1, 'u1', ?2, 'link', 'x', 'rejected', CURRENT_TIMESTAMP)",
                params![format!("h_{c}"), c],
            ).unwrap();
        }
        assert_eq!(keep_rate(&conn, "u1", 3).unwrap(), Some(0.0));
        assert!(matches!(run(&conn, "c4"), Outcome::AutoOff));
        let (v, source): (String, String) = conn
            .query_row("SELECT value_json, source FROM sleep_config WHERE user_id = 'u1' AND key = 'rem_auto_off'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((v.parse::<f64>().unwrap(), source.as_str()), (1.0, "fitted"));
        assert!(matches!(run(&conn, "c5"), Outcome::AutoOff), "stays off");
        // The user can turn it back on.
        config::set_owner(&conn, "u1", config::Group::Threshold, &[("rem_auto_off".to_string(), json!(0))]).unwrap();
        assert!(matches!(run(&conn, "c6"), Outcome::Ran(_)));
    }

    #[test]
    fn stage_off_rem_does_nothing() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        assert!(matches!(run_stage(&conn, "u1", "c1", chrono::Utc::now()).unwrap(), Outcome::Off));
    }
}
