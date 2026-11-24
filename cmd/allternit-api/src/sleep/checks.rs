//! Spaced checks and the tether ladder (step 6.3), the `checks` stage of a nap.
//!
//! A check is a deterministic test with no model call. It asks three things
//! of one current fact:
//!
//! - **(a) reachable**: the fact is in the top `checks_recall_k` hits when the
//!   index is searched for the fact's own words;
//! - **(b) consistent**: no newer current fact contradicts it through a
//!   `contradicts` edge unless a dispute record covers the pair;
//! - **(c) sourced**: its `source_observation_id` back-pointer still exists
//!   (a fact with no pointer passes).
//!
//! Score = checks passed / 3. `pass` = 3 of 3. `fail` = (a) or (b) failed
//! (the memory is unreachable or silently contradicted). Anything else is
//! `weak` (only (c) failed): recorded, no ladder move.
//!
//! **Spacing.** A fact's first check is due `checks_spacing_base_days` after it
//! was written. Each check leaves one pending `spaced` row (result null) with
//! the next due date: `base * growth^n` days after `n` consecutive passes
//! (capped at 365), or `base` days after a check that did not pass.
//!
//! **The bounded set** is built the same way for 60 or 6,000 memories. At most
//! `checks_max_per_cycle` facts, in this order, each step taking only what is
//! left: today's changes (facts written since the last finished nap, or the
//! last day; at most half), their neighbours (shared `memory_relationships`
//! edges, [`router_neighbours`] is the named hook for step 4.4), the due spaced
//! checks (oldest-due pool, ranked by faded retrieval strength and stakes), and
//! `checks_sample_size` slots of a fixed sample stratified by tether state and
//! age bucket. The general-skill probe slot is recorded `skipped` until pages
//! exist. Untethered facts are not checked (they are off the index).
//!
//! **Ladder.** `memory_facts.tether` (null = tethered): tethered, weak, gist,
//! untethered. A `fail` moves one step down and queues reconsolidation (a
//! `queued_reconsolidation` row; no recall-like boost for a memory that just
//! failed). A `pass` on a `weak` memory moves it one step up. `gist` only marks
//! the fact for the page builder (4.2) to use a shorter form. `untethered`
//! facts are dropped from recall ([`untethered_ids`], gated on this stage) but
//! never deleted: a direct load by id still returns them.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::config::Resolved;
use super::strength::{self, fmt_ts, parse_ts};
use super::types::Stage;
use crate::memory_index::{self, Scope};

pub const TETHERED: &str = "tethered";
pub const WEAK: &str = "weak";
pub const GIST: &str = "gist";
pub const UNTETHERED: &str = "untethered";

/// Spacing never waits longer than this many days.
const MAX_SPACING_DAYS: f64 = 365.0;

pub fn normalise(tether: Option<&str>) -> &'static str {
    match tether {
        Some(WEAK) => WEAK,
        Some(GIST) => GIST,
        Some(UNTETHERED) => UNTETHERED,
        _ => TETHERED,
    }
}

/// One step down the ladder (untethered stays).
pub fn step_down(t: &str) -> &'static str {
    match normalise(Some(t)) {
        TETHERED => WEAK,
        WEAK => GIST,
        _ => UNTETHERED,
    }
}

/// One step up after a pass: only a `weak` memory moves back.
pub fn step_up(t: &str) -> &'static str {
    match normalise(Some(t)) {
        WEAK => TETHERED,
        other => other,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub max_per_cycle: usize,
    pub sample_size: usize,
    pub spacing_base_days: f64,
    pub spacing_growth: f64,
    pub neighbours: usize,
    pub recall_k: usize,
}

impl Settings {
    pub fn from_config(cfg: &Resolved) -> Settings {
        let max_per_cycle = cfg.number("checks_max_per_cycle").max(1.0) as usize;
        Settings {
            max_per_cycle,
            sample_size: (cfg.number("checks_sample_size").max(0.0) as usize).min(max_per_cycle),
            spacing_base_days: cfg.number("checks_spacing_base_days").max(0.1),
            spacing_growth: cfg.number("checks_spacing_growth").max(1.0),
            neighbours: cfg.number("check_neighbours").max(0.0) as usize,
            recall_k: cfg.number("checks_recall_k").max(1.0) as usize,
        }
    }
}

/// Days until the next check after `consecutive_passes` passes in a row
/// (counting this one), or after a result that was not a pass (0).
pub fn spacing_days(s: &Settings, consecutive_passes: u32) -> f64 {
    if consecutive_passes == 0 {
        return s.spacing_base_days.min(MAX_SPACING_DAYS);
    }
    (s.spacing_base_days * s.spacing_growth.powi(consecutive_passes as i32)).min(MAX_SPACING_DAYS)
}

fn new_id() -> String {
    format!("chk_{}", uuid::Uuid::new_v4().simple())
}

// ── The check itself ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub reachable: bool,
    pub consistent: bool,
    pub sourced: bool,
}

impl Verdict {
    pub fn score(&self) -> f64 {
        [self.reachable, self.consistent, self.sourced].iter().filter(|b| **b).count() as f64 / 3.0
    }

    pub fn result(&self) -> &'static str {
        if self.reachable && self.consistent && self.sourced {
            "pass"
        } else if !self.reachable || !self.consistent {
            "fail"
        } else {
            "weak"
        }
    }
}

pub fn check_fact(conn: &Connection, user_id: &str, fact_id: &str, text: &str, source_observation_id: Option<&str>, recall_k: usize) -> rusqlite::Result<Verdict> {
    // (a) the fact's own words find it in the index (raw search: the tether filter is not applied here).
    let reachable = if memory_index::fts_query(text).is_none() {
        true
    } else {
        let q = memory_index::hash_embed(&[text.to_string()]);
        let scope = Scope { scope: user_id, target_types: &["fact"], only_ids: None };
        let hits = memory_index::hybrid_search(conn, &scope, text, Some((q.model.as_str(), q.vectors[0].as_slice())), recall_k)?;
        hits.iter().take(recall_k).any(|h| h.target_id == fact_id)
    };
    // (b) no current fact contradicts it without a dispute record.
    let undisputed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_relationships r
         JOIN memory_facts o ON o.user_id = r.user_id AND o.valid_until IS NULL
              AND o.id = CASE WHEN r.source_entity_id = ?2 THEN r.target_entity_id ELSE r.source_entity_id END
         WHERE r.user_id = ?1 AND r.relation_type = 'contradicts' AND r.source_kind = 'fact' AND r.target_kind = 'fact'
           AND r.valid_until IS NULL AND (r.source_entity_id = ?2 OR r.target_entity_id = ?2)
           AND NOT EXISTS (SELECT 1 FROM memory_disputes d WHERE d.user_id = r.user_id
               AND ((d.side_a_id = r.source_entity_id AND d.side_b_id = r.target_entity_id)
                 OR (d.side_a_id = r.target_entity_id AND d.side_b_id = r.source_entity_id)))",
        params![user_id, fact_id],
        |r| r.get(0),
    )?;
    // (c) the source back-pointer still exists.
    let sourced = match source_observation_id {
        None => true,
        Some(obs) => conn
            .query_row("SELECT 1 FROM memory_observations WHERE id = ?1", params![obs], |_| Ok(()))
            .optional()?
            .is_some(),
    };
    Ok(Verdict { reachable, consistent: undisputed == 0, sourced })
}

// ── The bounded check set ────────────────────────────────────────────────────

/// Hook for step 4.4: facts that share router cues or real co-load pairs with
/// the changed ones. The router is not built, so there are none yet.
pub fn router_neighbours(_conn: &Connection, _user_id: &str, _changed: &[String]) -> Vec<String> {
    Vec::new()
}

fn checkable(conn: &Connection, user_id: &str, id: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL
               AND COALESCE(tether, 'tethered') != 'untethered'",
            params![id, user_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Facts linked to `fact_id` by a fact-to-fact edge, or about the same entity.
fn relation_neighbours(conn: &Connection, user_id: &str, fact_id: &str, limit: usize) -> rusqlite::Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut edge = conn.prepare(
        "SELECT CASE WHEN source_entity_id = ?2 THEN target_entity_id ELSE source_entity_id END
         FROM memory_relationships
         WHERE user_id = ?1 AND source_kind = 'fact' AND target_kind = 'fact' AND valid_until IS NULL
           AND COALESCE(relation_type, '') != 'unrelated' AND (source_entity_id = ?2 OR target_entity_id = ?2)
         ORDER BY id LIMIT ?3",
    )?;
    let rows = edge.query_map(params![user_id, fact_id, limit as i64], |r| r.get::<_, String>(0))?;
    for r in rows {
        out.push(r?);
    }
    let mut shared = conn.prepare(
        "SELECT r2.source_entity_id FROM memory_relationships r1
         JOIN memory_relationships r2 ON r2.user_id = r1.user_id AND r2.relation_type = 'about_entity'
              AND r2.source_kind = 'fact' AND r2.target_entity_id = r1.target_entity_id
         WHERE r1.user_id = ?1 AND r1.relation_type = 'about_entity' AND r1.source_kind = 'fact'
           AND r1.source_entity_id = ?2 AND r2.source_entity_id != ?2
         ORDER BY r2.source_entity_id LIMIT ?3",
    )?;
    let rows = shared.query_map(params![user_id, fact_id, limit as i64], |r| r.get::<_, String>(0))?;
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// The finish time of the last nap, else one day back.
fn since(conn: &Connection, user_id: &str, cycle_id: &str, now: DateTime<Utc>) -> rusqlite::Result<DateTime<Utc>> {
    let last: Option<String> = conn.query_row(
        "SELECT MAX(finished_at) FROM sleep_cycles WHERE user_id = ?1 AND state = 'finished' AND id != ?2",
        params![user_id, cycle_id],
        |r| r.get(0),
    )?;
    Ok(last.as_deref().and_then(parse_ts).unwrap_or(now - Duration::days(1)))
}

fn fnv(seed: &str, n: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes().chain(n.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

struct Picker {
    picked: Vec<(String, &'static str)>,
    seen: HashSet<String>,
}

impl Picker {
    fn add(&mut self, id: String, kind: &'static str) -> bool {
        if self.seen.insert(id.clone()) {
            self.picked.push((id, kind));
            true
        } else {
            false
        }
    }
}

/// The ids to check this cycle and why. Never more than `max_per_cycle`.
pub fn select(conn: &Connection, user_id: &str, cfg: &Resolved, cycle_id: &str, now: DateTime<Utc>) -> rusqlite::Result<Vec<(String, &'static str)>> {
    let s = Settings::from_config(cfg);
    let sp = strength::Params::from_config(cfg);
    let mut p = Picker { picked: Vec::new(), seen: HashSet::new() };
    let main_slots = s.max_per_cycle - s.sample_size;

    // 1. Today's changes.
    let changed_cap = main_slots / 2;
    let since_ts = fmt_ts(since(conn, user_id, cycle_id, now)?);
    let mut changed: Vec<String> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT id FROM memory_facts WHERE user_id = ?1 AND valid_until IS NULL
               AND COALESCE(tether, 'tethered') != 'untethered' AND valid_from > ?2 AND valid_from <= ?3
             ORDER BY valid_from DESC, id LIMIT ?4",
        )?;
        let rows = st.query_map(params![user_id, since_ts, fmt_ts(now), changed_cap as i64], |r| r.get::<_, String>(0))?;
        for r in rows {
            let id = r?;
            if p.add(id.clone(), "changed") {
                changed.push(id);
            }
        }
    }

    // 2. Their neighbours: relation edges now, router neighbours through the hook.
    let neigh_cap = main_slots.saturating_sub(p.picked.len()) / 2;
    let mut got = 0usize;
    let mut candidates: Vec<String> = Vec::new();
    for id in &changed {
        candidates.extend(relation_neighbours(conn, user_id, id, s.neighbours)?);
    }
    candidates.extend(router_neighbours(conn, user_id, &changed));
    for id in candidates {
        if got >= neigh_cap {
            break;
        }
        if !p.seen.contains(&id) && checkable(conn, user_id, &id)? && p.add(id, "neighbour") {
            got += 1;
        }
    }

    // 3. Due spaced checks, ranked by how far retrieval has faded and by stakes.
    let due_cap = main_slots.saturating_sub(p.picked.len());
    if due_cap > 0 {
        let cutoff = fmt_ts(now - Duration::milliseconds((s.spacing_base_days * 86_400_000.0) as i64));
        let mut st = conn.prepare(
            "SELECT f.id, f.attention FROM memory_facts f
             LEFT JOIN sleep_checks c ON c.user_id = f.user_id AND c.target_kind = 'fact' AND c.target_id = f.id
                  AND c.check_kind = 'spaced' AND c.result IS NULL
             WHERE f.user_id = ?1 AND f.valid_until IS NULL AND COALESCE(f.tether, 'tethered') != 'untethered'
               AND ((c.id IS NULL AND f.valid_from <= ?2) OR (c.id IS NOT NULL AND c.due_at <= ?3))
             ORDER BY COALESCE(c.due_at, f.valid_from), f.id LIMIT ?4",
        )?;
        let pool: Vec<(String, Option<f64>)> = st
            .query_map(params![user_id, cutoff, fmt_ts(now), (due_cap * 4) as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let mut ranked: Vec<(f64, String)> = Vec::new();
        for (id, attention) in pool {
            if p.seen.contains(&id) {
                continue;
            }
            let rs = strength::current_retrieval(conn, &id, now, &sp)?.unwrap_or(0.5);
            let dispute: Option<f64> = conn.query_row(
                "SELECT MAX(stakes) FROM memory_disputes WHERE user_id = ?1 AND (side_a_id = ?2 OR side_b_id = ?2)",
                params![user_id, id],
                |r| r.get(0),
            )?;
            let stakes = attention.unwrap_or(0.0).max(dispute.unwrap_or(0.0));
            ranked.push((0.6 * (1.0 - rs) + 0.4 * stakes, id));
        }
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
        for (_, id) in ranked.into_iter().take(due_cap) {
            p.add(id, "spaced");
        }
    }

    // 4. The fixed stratified sample: tether state x age bucket, round-robin.
    if s.sample_size > 0 {
        let day = 86_400_000;
        let edges = [
            (fmt_ts(now - Duration::milliseconds(30 * day)), fmt_ts(now + Duration::days(1))),
            (fmt_ts(now - Duration::milliseconds(180 * day)), fmt_ts(now - Duration::milliseconds(30 * day))),
            ("0000-01-01 00:00:00".to_string(), fmt_ts(now - Duration::milliseconds(180 * day))),
        ];
        let fetch = s.sample_size + p.picked.len();
        let mut queues: Vec<Vec<String>> = Vec::new();
        for (ti, tether) in [TETHERED, WEAK, GIST].iter().enumerate() {
            for (ai, (from, to)) in edges.iter().enumerate() {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM memory_facts WHERE user_id = ?1 AND valid_until IS NULL
                       AND COALESCE(tether, 'tethered') = ?2 AND valid_from > ?3 AND valid_from <= ?4",
                    params![user_id, tether, from, to],
                    |r| r.get(0),
                )?;
                if n == 0 {
                    continue;
                }
                let span = (n as u64).saturating_sub(fetch as u64) + 1;
                let offset = fnv(cycle_id, (ti * 3 + ai) as u64) % span;
                let mut st = conn.prepare(
                    "SELECT id FROM memory_facts WHERE user_id = ?1 AND valid_until IS NULL
                       AND COALESCE(tether, 'tethered') = ?2 AND valid_from > ?3 AND valid_from <= ?4
                     ORDER BY id LIMIT ?5 OFFSET ?6",
                )?;
                let ids: Vec<String> = st
                    .query_map(params![user_id, tether, from, to, fetch as i64, offset as i64], |r| r.get(0))?
                    .collect::<Result<_, _>>()?;
                queues.push(ids);
            }
        }
        let mut taken = 0usize;
        let mut round = 0usize;
        while taken < s.sample_size && queues.iter().any(|q| round < q.len()) {
            for q in &queues {
                if taken >= s.sample_size {
                    break;
                }
                if let Some(id) = q.get(round) {
                    if p.add(id.clone(), "sample") {
                        taken += 1;
                    }
                }
            }
            round += 1;
        }
    }
    p.picked.truncate(s.max_per_cycle);
    Ok(p.picked)
}

// ── Running a cycle's checks ─────────────────────────────────────────────────

fn consecutive_passes(conn: &Connection, user_id: &str, fact_id: &str) -> rusqlite::Result<u32> {
    let mut st = conn.prepare(
        "SELECT result FROM sleep_checks WHERE user_id = ?1 AND target_kind = 'fact' AND target_id = ?2
           AND result IS NOT NULL AND check_kind != 'queued_reconsolidation'
         ORDER BY checked_at DESC, rowid DESC LIMIT 30",
    )?;
    let rows = st.query_map(params![user_id, fact_id], |r| r.get::<_, String>(0))?;
    let mut n = 0;
    for r in rows {
        if r? != "pass" {
            break;
        }
        n += 1;
    }
    Ok(n)
}

pub fn set_tether(conn: &Connection, user_id: &str, fact_id: &str, tether: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE memory_facts SET tether = ?3 WHERE id = ?1 AND user_id = ?2", params![fact_id, user_id, tether])?;
    Ok(())
}

/// Check one fact, record the row, move the ladder and schedule the next check.
fn run_one(conn: &Connection, user_id: &str, cycle_id: &str, s: &Settings, now: DateTime<Utc>, id: &str, kind: &str, tally: &mut Tally) -> rusqlite::Result<()> {
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT fact, source_observation_id, tether FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
            params![id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((text, source, tether)) = row else { return Ok(()) };
    let v = check_fact(conn, user_id, id, &text, source.as_deref(), s.recall_k)?;
    let result = v.result();
    let was = normalise(tether.as_deref());
    let prior = consecutive_passes(conn, user_id, id)?;

    // The pending spaced row is a schedule slot: replace it with the result and the next slot.
    let scheduled: Option<String> = conn
        .query_row(
            "SELECT due_at FROM sleep_checks WHERE user_id = ?1 AND target_kind = 'fact' AND target_id = ?2
               AND check_kind = 'spaced' AND result IS NULL",
            params![user_id, id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    conn.execute(
        "DELETE FROM sleep_checks WHERE user_id = ?1 AND target_kind = 'fact' AND target_id = ?2
           AND check_kind = 'spaced' AND result IS NULL",
        params![user_id, id],
    )?;
    conn.execute(
        "INSERT INTO sleep_checks (id, user_id, cycle_id, target_kind, target_id, check_kind, due_at, result, score, checked_at)
         VALUES (?1, ?2, ?3, 'fact', ?4, ?5, ?6, ?7, ?8, ?9)",
        params![new_id(), user_id, cycle_id, id, kind, scheduled, result, v.score(), fmt_ts(now)],
    )?;

    let now_tether = match result {
        "fail" => step_down(was),
        "pass" => step_up(was),
        _ => was,
    };
    if now_tether != was {
        set_tether(conn, user_id, id, now_tether)?;
    }
    if result == "fail" {
        conn.execute(
            "INSERT INTO sleep_checks (id, user_id, cycle_id, target_kind, target_id, check_kind, due_at, result, score, checked_at)
             VALUES (?1, ?2, ?3, 'fact', ?4, 'queued_reconsolidation', ?5, NULL, ?6, NULL)",
            params![new_id(), user_id, cycle_id, id, fmt_ts(now), v.score()],
        )?;
    }
    let passes = if result == "pass" { prior + 1 } else { 0 };
    let next = now + Duration::milliseconds((spacing_days(s, passes) * 86_400_000.0) as i64);
    if now_tether != UNTETHERED {
        conn.execute(
            "INSERT INTO sleep_checks (id, user_id, cycle_id, target_kind, target_id, check_kind, due_at, result, score, checked_at)
             VALUES (?1, ?2, NULL, 'fact', ?3, 'spaced', ?4, NULL, NULL, NULL)",
            params![new_id(), user_id, id, fmt_ts(next)],
        )?;
    }

    tally.checked += 1;
    *tally.by_result.entry(result).or_default() += 1;
    *tally.by_kind.entry(kind.to_string()).or_default() += 1;
    if now_tether != was {
        if result == "fail" { tally.moved_down += 1 } else { tally.moved_up += 1 }
    }
    Ok(())
}

#[derive(Default)]
struct Tally {
    checked: usize,
    moved_down: usize,
    moved_up: usize,
    by_result: BTreeMap<&'static str, usize>,
    by_kind: BTreeMap<String, usize>,
}

/// Run the `checks` stage for a user. `None` when the stage is off (nothing is read or written).
pub fn run_stage(conn: &Connection, user_id: &str, cycle_id: &str, now: DateTime<Utc>) -> rusqlite::Result<Option<Value>> {
    let cfg = super::config::load(conn, user_id)?;
    if !cfg.stage_on(Stage::Checks) {
        return Ok(None);
    }
    let s = Settings::from_config(&cfg);
    let set = select(conn, user_id, &cfg, cycle_id, now)?;
    let mut tally = Tally::default();
    for (id, kind) in &set {
        run_one(conn, user_id, cycle_id, &s, now, id, kind, &mut tally)?;
    }
    Ok(Some(json!({
        "checked": tally.checked,
        "max_per_cycle": s.max_per_cycle,
        "by_kind": tally.by_kind,
        "by_result": tally.by_result,
        "moved_down": tally.moved_down,
        "moved_up": tally.moved_up,
        "skill_probe": "skipped: no pages yet",
    })))
}

// ── Off the index ────────────────────────────────────────────────────────────

/// Which of these fact ids are untethered, for a user whose `checks` stage is
/// on. Empty (no query on `memory_facts`) when the stage is off, so recall is
/// unchanged for everyone else. Recall drops these; a direct load by id does not.
pub fn untethered_ids(conn: &Connection, user_id: &str, fact_ids: &[&str]) -> rusqlite::Result<HashSet<String>> {
    if fact_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let cfg = super::config::load(conn, user_id)?;
    if !cfg.stage_on(Stage::Checks) {
        return Ok(HashSet::new());
    }
    let marks = vec!["?"; fact_ids.len()].join(",");
    let sql = format!("SELECT id FROM memory_facts WHERE user_id = ?1 AND tether = 'untethered' AND id IN ({marks})");
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&user_id];
    for id in fact_ids {
        args.push(id);
    }
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?;
    let mut out = HashSet::new();
    for r in rows {
        out.insert(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use crate::memory_kernel_service as kernel;
    use super::super::config::{self, Group};

    fn db() -> DbHandle {
        let dir = std::env::temp_dir().join(format!("sleep-checks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        DbHandle::new(dir.join("t.db")).unwrap()
    }

    fn t0() -> DateTime<Utc> {
        parse_ts("2026-01-01 00:00:00").unwrap()
    }

    fn on(db: &DbHandle, numbers: &[(&str, f64)]) {
        let conn = db.connect().unwrap();
        let stage = vec![("enabled".to_string(), json!(true)), ("stage.checks".to_string(), json!(true))];
        config::set_owner(&conn, "u1", Group::Setting, &stage).unwrap();
        let nums: Vec<(String, Value)> = numbers.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        if !nums.is_empty() {
            config::set_owner(&conn, "u1", Group::Threshold, &nums).unwrap();
        }
    }

    /// Persist `texts` as facts written at `t0`.
    fn facts(db: &DbHandle, texts: &[String]) -> Vec<String> {
        let obs = kernel::record_observation(db, "u1", None, None, "turn_user", "seed", Some("user")).unwrap();
        let ids: Vec<String> = kernel::persist_facts(db, "u1", None, &obs, texts).unwrap().into_iter().map(|f| f.id).collect();
        db.connect().unwrap().execute("UPDATE memory_facts SET valid_from = ?1 WHERE user_id = 'u1'", [fmt_ts(t0())]).unwrap();
        ids
    }

    fn done(db: &DbHandle) -> i64 {
        db.connect().unwrap()
            .query_row("SELECT COUNT(*) FROM sleep_checks WHERE user_id = 'u1' AND result IS NOT NULL", [], |r| r.get(0))
            .unwrap()
    }

    fn tether(db: &DbHandle, id: &str) -> Option<String> {
        db.connect().unwrap().query_row("SELECT tether FROM memory_facts WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    fn run(db: &DbHandle, at: DateTime<Utc>, cycle: &str) -> Option<Value> {
        run_stage(&db.connect().unwrap(), "u1", cycle, at).unwrap()
    }

    #[test]
    fn an_unchanged_old_memory_is_checked_at_its_due_date() {
        let db = db();
        on(&db, &[("checks_sample_size", 0.0), ("checks_spacing_base_days", 3.0)]);
        let ids = facts(&db, &["Zorblat keeps quokka figurines".to_string()]);
        run(&db, t0() + Duration::days(2), "c1");
        assert_eq!(done(&db), 0, "not due yet");
        run(&db, t0() + Duration::days(3), "c2");
        assert_eq!(done(&db), 1, "checked on the due date");
        let (kind, result): (String, String) = db.connect().unwrap()
            .query_row("SELECT check_kind, result FROM sleep_checks WHERE target_id = ?1 AND result IS NOT NULL", [&ids[0]], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((kind.as_str(), result.as_str()), ("spaced", "pass"));
        // One pass: the next check waits base * growth = 6 days.
        run(&db, t0() + Duration::days(3) + Duration::days(5), "c3");
        assert_eq!(done(&db), 1, "next due date has not come");
        run(&db, t0() + Duration::days(3) + Duration::days(6), "c4");
        assert_eq!(done(&db), 2);
    }

    #[test]
    fn an_untethered_topic_is_off_the_index_but_found_by_a_direct_load() {
        let db = db();
        let ids = facts(&db, &["Zorblat keeps quokka figurines".to_string()]);
        db.connect().unwrap().execute("UPDATE memory_facts SET tether = 'untethered' WHERE id = ?1", [&ids[0]]).unwrap();
        let hits = |db: &DbHandle| kernel::recall(db, "u1", None, None, "quokka figurines", 5).unwrap();
        assert!(hits(&db).iter().any(|r| r.id == ids[0]), "stage off: recall is unchanged");
        on(&db, &[]);
        assert!(!hits(&db).iter().any(|r| r.id == ids[0]), "stage on: untethered is off the index");
        let loaded = kernel::get_fact(&db, "u1", &ids[0]).unwrap().expect("a direct load still finds it");
        assert!(loaded.content.contains("quokka"));
        assert!(kernel::get_fact(&db, "someone-else", &ids[0]).unwrap().is_none());
    }

    #[test]
    fn check_count_is_the_same_for_60_and_6000_memories() {
        let counts: Vec<i64> = [60usize, 6000]
            .iter()
            .map(|n| {
                let db = db();
                on(&db, &[("checks_max_per_cycle", 20.0), ("checks_sample_size", 4.0)]);
                let texts: Vec<String> = (0..*n).map(|i| format!("Item zq{i}x number {i} alpha beta")).collect();
                facts(&db, &texts);
                let detail = run(&db, t0() + Duration::days(30), "c1").unwrap();
                assert_eq!(detail["skill_probe"], "skipped: no pages yet");
                let real: i64 = db.connect().unwrap()
                    .query_row("SELECT COUNT(*) FROM sleep_checks WHERE result IS NOT NULL AND check_kind IN ('spaced','sample','changed','neighbour')", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(real, done(&db));
                real
            })
            .collect();
        assert_eq!(counts, vec![20, 20]);
    }

    #[test]
    fn a_fail_moves_one_step_down_and_a_later_pass_moves_one_step_up() {
        let db = db();
        on(&db, &[("checks_sample_size", 0.0), ("checks_spacing_base_days", 3.0)]);
        let ids = facts(&db, &["Zorblat keeps quokka figurines".to_string(), "Zorblat keeps no quokka figurines".to_string()]);
        // A silent contradiction: an edge with no dispute record.
        db.connect().unwrap().execute(
            "INSERT INTO memory_relationships (id, user_id, source_entity_id, target_entity_id, relation, relation_type, source_kind, target_kind)
             VALUES ('r1', 'u1', ?1, ?2, 'contradicts', 'contradicts', 'fact', 'fact')",
            params![ids[0], ids[1]],
        ).unwrap();
        run(&db, t0() + Duration::days(3), "c1");
        assert_eq!(tether(&db, &ids[0]).as_deref(), Some("weak"), "one step down");
        let queued: i64 = db.connect().unwrap()
            .query_row("SELECT COUNT(*) FROM sleep_checks WHERE target_id = ?1 AND check_kind = 'queued_reconsolidation'", [&ids[0]], |r| r.get(0)).unwrap();
        assert_eq!(queued, 1);
        // The conflict is cleared; the next check (3 days after a non-pass) passes and steps back up.
        db.connect().unwrap().execute("DELETE FROM memory_relationships WHERE id = 'r1'", []).unwrap();
        run(&db, t0() + Duration::days(6), "c2");
        assert_eq!(tether(&db, &ids[0]).as_deref(), Some("tethered"), "one step up");
    }

    #[test]
    fn stage_off_recall_unchanged_and_no_checks_written() {
        let db = db();
        let ids = facts(&db, &["Zorblat keeps quokka figurines".to_string()]);
        assert!(run(&db, t0() + Duration::days(30), "c1").is_none());
        assert_eq!(db.connect().unwrap().query_row("SELECT COUNT(*) FROM sleep_checks", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(tether(&db, &ids[0]), None);
        let before: Vec<String> = kernel::recall(&db, "u1", None, None, "quokka figurines", 5).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(before, vec![ids[0].clone()]);
    }
}
