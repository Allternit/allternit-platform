//! Fits Sleep's memory-decay parameter from the user's own labelled recalls (D9, step 3.4).
//!
//! Each labelled recall (`memory_recall_logs.label_used` not NULL) shows a
//! set of facts and says which the reply used. For every shown fact the age
//! is the days since that fact was last recalled (its `valid_from` the first
//! time), and the outcome is used / not used. The model is
//! `p_used = a * 0.5^(age / h)`.
//!
//! Method: grid search over `h` on a log grid, refined on a finer grid around
//! the best cell; for each `h` the best `a` in (0, 1/max weight) comes from
//! bisection on the (concave) Bernoulli log-likelihood derivative. No
//! randomness, no extra crates.
//!
//! Only `strength_half_life_days` is fitted. `strength_spacing_credit` is not
//! identifiable from used/not-used labels alone (it shapes storage strength,
//! which these labels never observe), so it keeps its default.
//!
//! Writes `sleep_config` rows with `source = 'fitted'` and never replaces an
//! `owner` row. Below `fit_min_labeled_events` it writes nothing. Nothing
//! calls this yet; the orchestrator (5.1) will.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::Value;

use super::config::{self, ConfigError};
use super::strength::parse_ts;

pub const KEY_HALF_LIFE: &str = "strength_half_life_days";
/// Log-likelihood gain over a no-decay model that a fit must show.
const MIN_LL_GAIN: f64 = 3.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FitReport {
    /// Labelled recalls used.
    pub events: usize,
    /// (key, value) rows written with `source = 'fitted'`.
    pub fitted: Vec<(String, f64)>,
    /// Keys fitted but kept because the user set them.
    pub kept_owner: Vec<String>,
    pub skipped_reason: Option<String>,
}

fn skip(events: usize, why: &str) -> FitReport {
    FitReport { events, skipped_reason: Some(why.to_string()), ..Default::default() }
}

/// (age in days, used) for every item shown in a labelled recall up to `now`.
fn collect(conn: &Connection, user_id: &str, now: DateTime<Utc>) -> rusqlite::Result<(usize, Vec<(f64, bool)>)> {
    let mut born: HashMap<String, DateTime<Utc>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT id, valid_from FROM memory_facts WHERE user_id = ?1")?;
        let rows = st.query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
        for row in rows {
            let (id, vf) = row?;
            if let Some(t) = vf.as_deref().and_then(parse_ts) {
                born.insert(id, t);
            }
        }
    }
    let mut st = conn.prepare(
        "SELECT created_at, results, label_used FROM memory_recall_logs
         WHERE user_id = ?1 AND label_used IS NOT NULL ORDER BY created_at ASC, id ASC",
    )?;
    let rows = st.query_map(params![user_id], |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?;
    let mut last: HashMap<String, DateTime<Utc>> = HashMap::new();
    let mut events = 0usize;
    let mut out = Vec::new();
    for row in rows {
        let (at, results, used) = row?;
        let Some(at) = at.as_deref().and_then(parse_ts) else { continue };
        if at > now {
            continue;
        }
        let (Ok(Value::Array(shown)), Ok(Value::Array(used))) =
            (serde_json::from_str::<Value>(&results), serde_json::from_str::<Value>(&used))
        else {
            continue;
        };
        events += 1;
        let used: Vec<&str> = used.iter().filter_map(|v| v.as_str()).collect();
        for item in &shown {
            let Some(id) = item.get("id").and_then(|v| v.as_str()) else { continue };
            if let Some(since) = last.get(id).or_else(|| born.get(id)) {
                let age = ((at - *since).num_milliseconds() as f64 / 86_400_000.0).max(0.0);
                out.push((age, used.contains(&id)));
            }
            last.insert(id.to_string(), at);
        }
    }
    Ok((events, out))
}

/// Best `a` for fixed weights `w = 0.5^(age/h)`, and its log-likelihood.
fn best_a(w: &[f64], y: &[bool]) -> (f64, f64) {
    let wmax = w.iter().cloned().fold(0.0_f64, f64::max).max(1e-12);
    let (mut lo, mut hi) = (1e-6, (1.0 / wmax).min(1.0) - 1e-9);
    if hi <= lo {
        return (lo, f64::NEG_INFINITY);
    }
    for _ in 0..50 {
        let a = 0.5 * (lo + hi);
        let d: f64 = w.iter().zip(y).map(|(wi, yi)| if *yi { 1.0 / a } else { -wi / (1.0 - a * wi) }).sum();
        if d > 0.0 { lo = a } else { hi = a }
    }
    let a = 0.5 * (lo + hi);
    (a, loglik(a, w, y))
}

fn loglik(a: f64, w: &[f64], y: &[bool]) -> f64 {
    w.iter().zip(y).map(|(wi, yi)| {
        let p = (a * wi).clamp(1e-12, 1.0 - 1e-12);
        if *yi { p.ln() } else { (1.0 - p).ln() }
    }).sum()
}

fn at_h(h: f64, ages: &[f64], y: &[bool]) -> (f64, f64) {
    let w: Vec<f64> = ages.iter().map(|g| 0.5_f64.powf(g / h)).collect();
    best_a(&w, y)
}

/// Fits `(h, a)`; returns `None` when decay explains the labels no better than a constant rate.
pub fn fit_decay(samples: &[(f64, bool)], h_min: f64, h_max: f64) -> Option<(f64, f64)> {
    if samples.len() < 2 || !samples.iter().any(|s| s.1) || samples.iter().all(|s| s.1) {
        return None;
    }
    let ages: Vec<f64> = samples.iter().map(|s| s.0).collect();
    let y: Vec<bool> = samples.iter().map(|s| s.1).collect();
    let rate = y.iter().filter(|b| **b).count() as f64 / y.len() as f64;
    let flat = loglik(rate, &vec![1.0; y.len()], &y);
    let ratio = (h_max / h_min).ln();
    let n = 120;
    let mut best = (h_min, f64::NEG_INFINITY, 0.0);
    for i in 0..=n {
        let h = h_min * (ratio * i as f64 / n as f64).exp();
        let (a, ll) = at_h(h, &ages, &y);
        if ll > best.1 {
            best = (h, ll, a);
        }
    }
    let step = ratio / n as f64;
    let centre = best.0.ln();
    for i in 0..=40 {
        let h = (centre - step + 2.0 * step * i as f64 / 40.0).exp().clamp(h_min, h_max);
        let (a, ll) = at_h(h, &ages, &y);
        if ll > best.1 {
            best = (h, ll, a);
        }
    }
    (best.1 - flat >= MIN_LL_GAIN).then_some((best.0, best.2))
}

/// Fits what the labels support for one user and writes `fitted` rows.
pub fn fit_user(conn: &Connection, user_id: &str, now: DateTime<Utc>) -> Result<FitReport, ConfigError> {
    let cfg = config::load(conn, user_id)?;
    let min_events = cfg.number("fit_min_labeled_events") as usize;
    let (events, samples) = collect(conn, user_id, now)?;
    if events < min_events {
        return Ok(skip(events, "too_few_labels"));
    }
    let d = config::def(KEY_HALF_LIFE).expect("half-life def");
    let Some((h, _a)) = fit_decay(&samples, d.min, d.max) else {
        return Ok(skip(events, "no_decay_signal"));
    };
    let written = config::set_fitted(conn, user_id, &[(KEY_HALF_LIFE, h)])?;
    let mut report = FitReport { events, ..Default::default() };
    if written.iter().any(|k| k == KEY_HALF_LIFE) {
        report.fitted.push((KEY_HALF_LIFE.to_string(), h));
    } else {
        report.kept_owner.push(KEY_HALF_LIFE.to_string());
        report.skipped_reason = Some("owner_value_kept".to_string());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use crate::sleep::strength::fmt_ts;
    use chrono::Duration;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
    }

    fn now() -> DateTime<Utc> {
        parse_ts("2026-06-01 00:00:00").unwrap()
    }

    /// `logs` labelled recalls of 5 facts each, ages 0..90 days, planted decay.
    fn seed(conn: &Connection, logs: usize, half_life: f64) {
        let mut rng = Lcg(42);
        for i in 0..logs {
            let mut shown = Vec::new();
            let mut used = Vec::new();
            for j in 0..5 {
                let id = format!("f{i}_{j}");
                let age = rng.next() * 90.0;
                let born = now() - Duration::milliseconds((age * 86_400_000.0) as i64);
                conn.execute(
                    "INSERT INTO memory_facts (id, user_id, fact, confidence, valid_from) VALUES (?1, 'u1', 'x', 0.6, ?2)",
                    params![id, fmt_ts(born)],
                )
                .unwrap();
                // Stored ages are whole seconds; recompute p from the stored age.
                let stored = (now() - parse_ts(&fmt_ts(born)).unwrap()).num_seconds() as f64 / 86_400.0;
                let p = 0.9 * 0.5_f64.powf(stored / half_life);
                if rng.next() < p {
                    used.push(id.clone());
                }
                shown.push(serde_json::json!({"id": id, "item_type": "fact"}));
            }
            conn.execute(
                "INSERT INTO memory_recall_logs (id, user_id, query, results, created_at, label_used, label_source, labeled_at)
                 VALUES (?1, 'u1', 'q', ?2, ?3, ?4, 'answer', ?3)",
                params![format!("l{i}"), Value::Array(shown).to_string(), fmt_ts(now()), serde_json::json!(used).to_string()],
            )
            .unwrap();
        }
    }

    fn row(conn: &Connection) -> Option<(f64, String)> {
        conn.query_row(
            "SELECT value_json, source FROM sleep_config WHERE user_id = 'u1' AND key = ?1",
            params![KEY_HALF_LIFE],
            |r| Ok((r.get::<_, String>(0)?.parse::<f64>().unwrap(), r.get(1)?)),
        )
        .ok()
    }

    #[test]
    fn a_synthetic_log_recovers_a_planted_decay_within_tolerance() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        seed(&conn, 800, 21.0);
        let rep = fit_user(&conn, "u1", now() + Duration::days(1)).unwrap();
        assert_eq!(rep.skipped_reason, None);
        assert_eq!(rep.events, 800);
        let h = rep.fitted.iter().find(|(k, _)| k == KEY_HALF_LIFE).expect("fitted half-life").1;
        assert!((h - 21.0).abs() / 21.0 <= 0.20, "fitted {h}");
        let (stored, source) = row(&conn).expect("fitted row");
        assert_eq!(source, "fitted");
        assert!((stored - h).abs() < 1e-9);
        // The resolver now reports it as fitted.
        let cfg = config::load(&conn, "u1").unwrap();
        assert_eq!(cfg.entries[KEY_HALF_LIFE].source, config::Source::Fitted);
    }

    #[test]
    fn an_owner_row_is_not_overwritten() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        seed(&conn, 800, 21.0);
        config::set_owner(
            &conn,
            "u1",
            config::Group::Threshold,
            &[(KEY_HALF_LIFE.to_string(), serde_json::json!(45.0))],
        )
        .unwrap();
        let rep = fit_user(&conn, "u1", now() + Duration::days(1)).unwrap();
        assert!(rep.fitted.is_empty());
        assert_eq!(rep.kept_owner, vec![KEY_HALF_LIFE.to_string()]);
        assert_eq!(row(&conn), Some((45.0, "owner".to_string())));
    }

    #[test]
    fn too_few_labels_writes_nothing() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        seed(&conn, 50, 21.0);
        let rep = fit_user(&conn, "u1", now() + Duration::days(1)).unwrap();
        assert_eq!(rep.skipped_reason.as_deref(), Some("too_few_labels"));
        assert_eq!(rep.events, 50);
        assert!(rep.fitted.is_empty());
        assert_eq!(row(&conn), None);
    }
}
