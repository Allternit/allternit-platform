//! Decision-head override: re-decide a run's recorded decisions with a
//! candidate head and compare it with what was served and what turned out
//! to be right.
//!
//! A run's decisions are the `decisions` rows of the run's owner whose
//! `session_id` (or `task`) is the run id. Each one is re-decided with the
//! candidate head as the head stands now, **minus the decision's own
//! outcome** (leave-one-out: the lookup table's count from this decision is
//! taken out inside a savepoint that is rolled back), so a head never gets
//! credit for remembering the answer it is being tested on. Nothing is
//! written: no shadow row, no lookup update.
//!
//! The candidate's *effective* choice is what the chain would serve with the
//! head live: its answer when its confidence clears the kind's threshold,
//! otherwise the recorded served choice (the chain escalates past the head).

use crate::agency_api::decisions::flywheel::{truth, Head, HeadInput, Truth};
use crate::agency_api::decisions::{acceptance, threshold};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Instant;

/// One recorded decision of a run.
#[derive(Clone, Debug, Default)]
pub struct Recorded {
    pub id: String,
    pub kind: String,
    pub served: Option<String>,
    pub backend: String,
    pub status: Option<String>,
    pub label: Option<String>,
    pub latency_ms: f64,
    pub cost_usd: f64,
    pub ids: Vec<String>,
    pub has_image: bool,
    pub key: Option<String>,
    pub learned: bool,
}

impl Recorded {
    /// The right option, when the outcome names one (`flywheel::truth`).
    pub fn right(&self) -> Option<String> {
        match truth(self.served.as_deref(), self.status.as_deref(), self.label.as_deref()) {
            Truth::Right(t) => Some(t),
            _ => None,
        }
    }
}

pub fn load(conn: &Connection, owner: &str, run_id: &str) -> rusqlite::Result<Vec<Recorded>> {
    let mut q = conn.prepare(
        "SELECT d.id, d.kind, d.choice, d.backend, d.outcome_status, d.outcome_label, d.latency_ms, d.attempts_json, d.options_json,
                d.has_image, k.key_hash, COALESCE(k.learned, 0)
         FROM decisions d LEFT JOIN decision_keys k ON k.decision_id = d.id
         WHERE d.owner = ?1 AND (d.session_id = ?2 OR d.task = ?2) ORDER BY d.created_at, d.id",
    )?;
    let rows = q.query_map(params![owner, run_id], |r| {
        let attempts: Value = serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(Value::Null);
        let options: Value = serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or(Value::Null);
        Ok(Recorded {
            id: r.get(0)?,
            kind: r.get(1)?,
            served: r.get(2)?,
            backend: r.get(3)?,
            status: r.get(4)?,
            label: r.get(5)?,
            latency_ms: r.get(6)?,
            cost_usd: attempts.as_array().map(|a| a.iter().filter_map(|x| x["detail"]["cost_usd"].as_f64()).sum()).unwrap_or(0.0),
            ids: options.as_array().map(|a| a.iter().filter_map(|o| o["id"].as_str().map(String::from)).collect()).unwrap_or_default(),
            has_image: r.get(9)?,
            key: r.get(10)?,
            learned: r.get(11)?,
        })
    })?;
    rows.collect()
}

/// The candidate's samples for one decision: `(choice index, confidence)`
/// or `None` (no answer), plus the head's mean latency.
pub struct Redecided {
    pub samples: Vec<Option<(usize, f64)>>,
    pub latency_ms: f64,
}

/// Re-decide `d` with `head`, leave-one-out, `n` times (1 for a deterministic head).
pub fn redecide(conn: &Connection, owner: &str, d: &Recorded, head: &dyn Head, n: u32) -> rusqlite::Result<Redecided> {
    let n = if head.deterministic() { 1 } else { n.max(1) };
    // Heads are blind to images and need the decision's key.
    let Some(key) = d.key.as_deref().filter(|_| !d.has_image) else {
        return Ok(Redecided { samples: vec![None; n as usize], latency_ms: 0.0 });
    };
    conn.execute_batch("SAVEPOINT replay_loo")?;
    let res = (|| -> rusqlite::Result<Redecided> {
        if d.learned {
            let (choice, col) = match truth(d.served.as_deref(), d.status.as_deref(), d.label.as_deref()) {
                Truth::Right(c) => (Some(c), "successes"),
                Truth::Wrong(c) => (Some(c), "failures"),
                Truth::Unknown => (None, ""),
            };
            if let Some(c) = choice {
                conn.execute(
                    &format!("UPDATE decision_lookup SET {col} = MAX({col} - 1, 0) WHERE owner = ?1 AND kind = ?2 AND key_hash = ?3 AND choice = ?4"),
                    params![owner, d.kind, key, c],
                )?;
            }
        }
        let ids: Vec<&str> = d.ids.iter().map(String::as_str).collect();
        let input = HeadInput { owner, kind: &d.kind, key, ids: &ids };
        let mut samples = Vec::with_capacity(n as usize);
        let t = Instant::now();
        for _ in 0..n {
            samples.push(head.predict(conn, &input)?.map(|a| (a.choice, a.probs.get(a.choice).copied().unwrap_or(0.0))));
        }
        Ok(Redecided { samples, latency_ms: t.elapsed().as_secs_f64() * 1000.0 / n as f64 })
    })();
    // Always undone: a replay never teaches the head.
    conn.execute_batch("ROLLBACK TO replay_loo; RELEASE replay_loo")?;
    res
}

/// One re-decided decision, for aggregation.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub kind: String,
    pub head: String,
    pub version: String,
    pub deterministic: bool,
    pub answered: bool,
    pub confident: bool,
    pub has_served: bool,
    pub agrees: bool,
    /// Labelled: `Some(right option)`.
    pub labelled: bool,
    pub correct: bool,
    pub served_correct: bool,
    pub head_correct: Option<bool>,
    /// (samples, correct samples) of the effective choice, labelled only.
    pub samples: (u32, u32),
    pub cost_usd: f64,
    pub baseline_cost_usd: f64,
    pub latency_ms: f64,
    pub baseline_latency_ms: f64,
}

/// Compare one decision with its re-decision: the per-decision record and its row.
pub fn compare(d: &Recorded, head: &dyn Head, r: &Redecided) -> (Value, Row) {
    let thr = threshold(&d.kind);
    let pick = |s: &Option<(usize, f64)>| -> Option<String> {
        match s {
            Some((i, c)) if *c >= thr => d.ids.get(*i).cloned(),
            _ => d.served.clone(),
        }
    };
    let first = r.samples.first().cloned().flatten();
    let answered = first.is_some();
    let confident = first.is_some_and(|(_, c)| c >= thr);
    let effective = pick(&r.samples.first().cloned().flatten());
    let right = d.right();
    let correct = right.is_some() && effective == right;
    let served_correct = right.is_some() && d.served == right;
    let head_choice = first.and_then(|(i, _)| d.ids.get(i).cloned());
    let row = Row {
        kind: d.kind.clone(),
        head: head.name().into(),
        version: head.version().into(),
        deterministic: head.deterministic(),
        answered,
        confident,
        has_served: d.served.is_some(),
        agrees: d.served.is_some() && effective == d.served,
        labelled: right.is_some(),
        correct,
        served_correct,
        head_correct: right.as_ref().and(head_choice.as_ref()).map(|_| head_choice == right),
        samples: if right.is_some() {
            (r.samples.len() as u32, r.samples.iter().filter(|s| pick(s) == right).count() as u32)
        } else {
            (0, 0)
        },
        cost_usd: if confident { 0.0 } else { d.cost_usd },
        baseline_cost_usd: d.cost_usd,
        latency_ms: r.latency_ms + if confident { 0.0 } else { d.latency_ms },
        baseline_latency_ms: d.latency_ms,
    };
    let v = json!({
        "decision_id": d.id,
        "kind": d.kind,
        "served": { "choice": d.served, "backend": d.backend, "latency_ms": d.latency_ms, "cost_usd": d.cost_usd },
        "outcome": d.status.as_ref().map(|s| json!({ "status": s, "label": d.label })),
        "label": right,
        "candidate": { "head": head.name(), "version": head.version(), "choice": head_choice,
                       "confidence": first.map(|(_, c)| (c * 10_000.0).round() / 10_000.0), "answered": answered,
                       "confident": confident, "latency_ms": r.latency_ms, "samples": r.samples.len() },
        "effective_choice": effective,
        "agrees_with_served": row.agrees,
        "correct": right.is_some().then_some(correct),
        "served_correct": right.is_some().then_some(served_correct),
    });
    (v, row)
}

fn ratio(a: u64, b: u64) -> Option<f64> {
    (b > 0).then(|| a as f64 / b as f64)
}

fn mean(xs: impl Iterator<Item = f64>) -> Option<f64> {
    let v: Vec<f64> = xs.collect();
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// Aggregate decision metrics over `rows` (one kind or all), pass@k with `k`.
pub fn metrics(rows: &[Row], k: u32) -> Value {
    let n = rows.len() as u64;
    let count = |f: &dyn Fn(&Row) -> bool| rows.iter().filter(|r| f(r)).count() as u64;
    let labelled = count(&|r| r.labelled);
    let head_answered_labelled = count(&|r| r.head_correct.is_some());
    let samples: Vec<(u32, u32)> = rows.iter().filter(|r| r.labelled).map(|r| r.samples).collect();
    let (p1, pk) = acceptance::pass_at(&samples, k);
    json!({
        "replayed": n,
        "answered": count(&|r| r.answered),
        "coverage": ratio(count(&|r| r.answered), n),
        "confident": count(&|r| r.confident),
        "agreement_with_served": ratio(count(&|r| r.agrees), count(&|r| r.has_served)),
        "labelled": labelled,
        "accuracy": ratio(count(&|r| r.correct), labelled),
        "head_accuracy": ratio(count(&|r| r.head_correct == Some(true)), head_answered_labelled),
        "baseline_accuracy": ratio(count(&|r| r.served_correct), labelled),
        "cost_usd": mean(rows.iter().map(|r| r.cost_usd)),
        "baseline_cost_usd": mean(rows.iter().map(|r| r.baseline_cost_usd)),
        "latency_ms": mean(rows.iter().map(|r| r.latency_ms)),
        "baseline_latency_ms": mean(rows.iter().map(|r| r.baseline_latency_ms)),
        "pass_at_1": p1,
        "pass_at_k": pk,
        "k": k,
        "deterministic": rows.iter().all(|r| r.deterministic),
    })
}

/// Metrics for all rows plus one block per kind (with its head and version).
pub fn metrics_by_kind(rows: &[Row], k_of: &dyn Fn(&str) -> u32) -> Value {
    let mut kinds: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for r in rows {
        kinds.entry(r.kind.clone()).or_default().push(r.clone());
    }
    let by_kind: serde_json::Map<String, Value> = kinds
        .iter()
        .map(|(kind, rs)| {
            let mut m = metrics(rs, k_of(kind));
            m["head"] = json!(rs[0].head);
            m["version"] = json!(rs[0].version);
            (kind.clone(), m)
        })
        .collect();
    let k = kinds.keys().map(|kd| k_of(kd)).max().unwrap_or(1);
    let mut all = metrics(rows, k);
    all["by_kind"] = Value::Object(by_kind);
    all
}
