//! Two strengths per memory (step 3.2, D9). Stage flag: `strengths`.
//!
//! - `storage_strength`: how well learned. It only ever rises.
//! - `retrieval_strength`: how reachable right now. It fades with time and a
//!   recall restores it to 1.0.
//!
//! Time lowers retrieval strength only. Nothing is deleted or retired for age.
//! A recall is a reconsolidation: it raises storage strength by an amount that
//! grows with the spacing since the last recall and with the difficulty (low
//! retrieval strength at recall time), then resets retrieval strength.
//!
//! The model (all numbers come from `sleep_config`, source `default` until the
//! fitter in step 3.4 replaces them):
//!
//! ```text
//! half_life(S)         = base_half_life_days * (1 + S)
//! retrieval_now(R,d,S) = R * 0.5 ^ (d / half_life(S))
//! gain                 = spacing_credit * min(1, d / min_spacing_days)
//!                        * (1 + difficulty_weight * (1 - retrieval_now))
//! gain                 = min(gain, instant_credit_cap)   when d < min_spacing_days
//! S'                   = S + gain          R' = 1.0
//! ```
//!
//! Every function takes `now`, so tests drive the clock without sleeping.
//! NULL strengths mean "never scored": the first recall that touches the fact
//! with the flag on initialises them from confidence and retrieval count.

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::config::Resolved;

pub const KEY_HALF_LIFE_DAYS: &str = "strength_half_life_days";
pub const KEY_SPACING_CREDIT: &str = "strength_spacing_credit";
pub const KEY_DIFFICULTY_WEIGHT: &str = "strength_difficulty_weight";
pub const KEY_MIN_SPACING_DAYS: &str = "strength_min_spacing_days";
pub const KEY_INSTANT_CREDIT_CAP: &str = "strength_instant_credit_cap";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub base_half_life_days: f64,
    pub spacing_credit: f64,
    pub difficulty_weight: f64,
    pub min_spacing_days: f64,
    pub instant_credit_cap: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            base_half_life_days: 30.0,
            spacing_credit: 0.2,
            difficulty_weight: 1.0,
            min_spacing_days: 1.0,
            instant_credit_cap: 0.02,
        }
    }
}

impl Params {
    pub fn from_config(cfg: &Resolved) -> Params {
        let d = Params::default();
        let pick = |key: &str, fallback: f64| {
            let v = cfg.number(key);
            if v > 0.0 { v } else { fallback }
        };
        Params {
            base_half_life_days: pick(KEY_HALF_LIFE_DAYS, d.base_half_life_days),
            spacing_credit: pick(KEY_SPACING_CREDIT, d.spacing_credit),
            difficulty_weight: cfg.number(KEY_DIFFICULTY_WEIGHT).max(0.0),
            min_spacing_days: pick(KEY_MIN_SPACING_DAYS, d.min_spacing_days),
            instant_credit_cap: cfg.number(KEY_INSTANT_CREDIT_CAP).max(0.0),
        }
    }
}

/// Half-life in days: well-learned memories fade slower.
pub fn half_life(p: &Params, storage: f64) -> f64 {
    p.base_half_life_days * (1.0 + storage.max(0.0))
}

/// Retrieval strength `days_since` days after it was `rs_last`.
pub fn retrieval_now(p: &Params, rs_last: f64, days_since: f64, storage: f64) -> f64 {
    let d = days_since.max(0.0);
    (rs_last * 0.5f64.powf(d / half_life(p, storage))).clamp(0.0, 1.0)
}

/// Storage strength gained by one recall.
pub fn recall_gain(p: &Params, days_since_last: f64, retrieval_at_recall: f64) -> f64 {
    let d = days_since_last.max(0.0);
    let spacing = (d / p.min_spacing_days).min(1.0);
    let difficulty = 1.0 + p.difficulty_weight * (1.0 - retrieval_at_recall.clamp(0.0, 1.0));
    let mut gain = p.spacing_credit * spacing * difficulty;
    if d < p.min_spacing_days {
        gain = gain.min(p.instant_credit_cap);
    }
    gain
}

/// Starting storage strength for a fact that has none yet.
pub fn initial_storage(confidence: f64, retrieval_count: i64) -> f64 {
    confidence.clamp(0.0, 1.0) * 0.5 + 0.1 * ((1 + retrieval_count.max(0)) as f64).ln()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strengths {
    pub storage: f64,
    pub retrieval: f64,
}

/// One recall: returns the new strengths. `rs_last`/`storage` are the stored
/// values, `days_since_last` the time since they were written.
pub fn recall(p: &Params, storage: f64, rs_last: f64, days_since_last: f64) -> Strengths {
    let now_rs = retrieval_now(p, rs_last, days_since_last, storage);
    let gain = recall_gain(p, days_since_last, now_rs);
    Strengths { storage: storage + gain.max(0.0), retrieval: 1.0 }
}

/// Reads SQLite (`YYYY-MM-DD HH:MM:SS`) and RFC 3339 timestamps.
pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(n.and_utc());
        }
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)).map(|n| n.and_utc())
}

pub fn fmt_ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// A recall touched these facts at `now`: reconsolidate each one. Call it
/// before the legacy `note_retrieved`, which overwrites `last_retrieved_at`.
/// Only for users whose `strengths` stage is on.
pub fn reconsolidate_at(
    conn: &Connection,
    user_id: &str,
    fact_ids: &[&str],
    now: DateTime<Utc>,
    p: &Params,
) -> rusqlite::Result<usize> {
    let mut sel = conn.prepare(
        "SELECT storage_strength, retrieval_strength, confidence, retrieval_count,
                COALESCE(last_reconsolidated_at, last_retrieved_at, valid_from)
         FROM memory_facts WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut upd = conn.prepare(
        "UPDATE memory_facts SET storage_strength = ?3, retrieval_strength = ?4, last_reconsolidated_at = ?5
         WHERE id = ?1 AND user_id = ?2",
    )?;
    let mut touched = 0;
    for id in fact_ids {
        let row = sel
            .query_row(params![id, user_id], |r| {
                Ok((
                    r.get::<_, Option<f64>>(0)?,
                    r.get::<_, Option<f64>>(1)?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .optional()?;
        let Some((storage, rs, conf, count, anchor)) = row else { continue };
        let last = anchor.as_deref().and_then(parse_ts).unwrap_or(now);
        let days = ((now - last).num_milliseconds() as f64 / 86_400_000.0).max(0.0);
        let s0 = storage.unwrap_or_else(|| initial_storage(conf, count));
        let r0 = rs.unwrap_or(1.0);
        let next = recall(p, s0, r0, days);
        upd.execute(params![id, user_id, next.storage, next.retrieval, fmt_ts(now)])?;
        touched += 1;
    }
    Ok(touched)
}

/// Recall hook: does nothing unless the user's `strengths` stage is on.
pub fn on_recall(conn: &Connection, user_id: &str, fact_ids: &[&str]) -> rusqlite::Result<usize> {
    if fact_ids.is_empty() {
        return Ok(0);
    }
    let cfg = super::config::load(conn, user_id)?;
    if !cfg.stage_on(super::types::Stage::Strengths) {
        return Ok(0);
    }
    reconsolidate_at(conn, user_id, fact_ids, Utc::now(), &Params::from_config(&cfg))
}

/// Current retrieval strength of one fact at `now` (no write). None while the
/// fact has never been scored.
pub fn current_retrieval(conn: &Connection, fact_id: &str, now: DateTime<Utc>, p: &Params) -> rusqlite::Result<Option<f64>> {
    let row = conn
        .query_row(
            "SELECT storage_strength, retrieval_strength, last_reconsolidated_at FROM memory_facts WHERE id = ?1",
            params![fact_id],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<String>>(2)?)),
        )
        .optional()?;
    Ok(row.and_then(|(s, rs, at)| {
        let (s, rs, at) = (s?, rs?, parse_ts(&at?)?);
        Some(retrieval_now(p, rs, (now - at).num_milliseconds() as f64 / 86_400_000.0, s))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use chrono::Duration;

    fn t0() -> DateTime<Utc> {
        parse_ts("2026-01-01 00:00:00").unwrap()
    }

    fn add(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO memory_facts (id, user_id, fact, confidence, valid_from) VALUES (?1, 'u1', 'x', 0.6, ?2)",
            params![id, fmt_ts(t0())],
        )
        .unwrap();
    }

    fn storage(conn: &Connection, id: &str) -> Option<f64> {
        conn.query_row("SELECT storage_strength FROM memory_facts WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn a_spaced_recalled_fact_has_higher_storage_strength_than_a_same_age_unused_one() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        add(&conn, "recalled");
        add(&conn, "unused");
        let p = Params::default();
        for day in [0, 3, 10] {
            reconsolidate_at(&conn, "u1", &["recalled"], t0() + Duration::days(day), &p).unwrap();
        }
        let recalled = storage(&conn, "recalled").unwrap();
        let baseline = initial_storage(0.6, 0);
        assert!(recalled > baseline, "{recalled} should beat the unused baseline {baseline}");
        assert_eq!(storage(&conn, "unused"), None, "untouched facts stay unscored");
        // Storage never goes down, and retrieval is reset.
        let again = reconsolidate_at(&conn, "u1", &["recalled"], t0() + Duration::days(10) + Duration::seconds(1), &p).unwrap();
        assert_eq!(again, 1);
        assert!(storage(&conn, "recalled").unwrap() >= recalled);
        let rs: f64 = conn.query_row("SELECT retrieval_strength FROM memory_facts WHERE id = 'recalled'", [], |r| r.get(0)).unwrap();
        assert_eq!(rs, 1.0);
    }

    #[test]
    fn an_instant_re_recall_earns_less_than_the_configured_credit() {
        let p = Params::default();
        let instant = recall_gain(&p, 0.0, 1.0);
        let seconds_later = recall_gain(&p, 1.0 / 86_400.0, 0.2);
        assert!(instant < p.spacing_credit);
        assert!(seconds_later <= p.instant_credit_cap && seconds_later < p.spacing_credit);
        let spaced = recall_gain(&p, 5.0, 1.0);
        assert!(spaced >= p.spacing_credit, "a spaced recall earns the full credit");
        // Harder (more faded) recalls earn more than easy ones at the same spacing.
        assert!(recall_gain(&p, 5.0, 0.2) > recall_gain(&p, 5.0, 0.9));
    }

    #[test]
    fn time_lowers_retrieval_only_and_well_learned_fades_slower() {
        let p = Params::default();
        let weak = retrieval_now(&p, 1.0, 30.0, 0.0);
        let strong = retrieval_now(&p, 1.0, 30.0, 3.0);
        assert!((weak - 0.5).abs() < 1e-9);
        assert!(strong > weak);
    }
}
