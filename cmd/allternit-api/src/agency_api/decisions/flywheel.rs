//! The decision flywheel (decision-runtime spec §2.4, phase E5): heads that
//! earn their place on live traffic.
//!
//! - A **head** is a tiny, fast decider registered per decision kind. Every
//!   decision of that kind runs it in **shadow**, off the latency path, after
//!   the served answer is stored: its choice lands in `decision_shadow`, next
//!   to the served choice (`decisions.choice`) and, later, the outcome.
//! - Per head and kind the flywheel keeps coverage, agreement with the served
//!   choice, accuracy against outcomes and an overconfidence gap. A shadow
//!   head is **promoted** to live when its accuracy clears the kind's bar over
//!   a minimum labelled sample; a live head is **demoted** back to shadow when
//!   its live accuracy drops under a lower bar. Metrics count only rows since
//!   the last transition, so each state has to be re-earned (hysteresis).
//!   Every transition, automatic or manual, is written to
//!   `decision_head_audit`.
//! - A **live** head is tried first in the chain (the `head` slot of
//!   `ALLTERNIT_DECISIONS_CHAIN`); a miss or low confidence escalates as usual.
//!
//! The first head is `head.lookup`: an exact-match lookup over the store
//! (same owner, kind, context hash, question and options → the choice that
//! succeeded there before). It needs no training job and answers from one
//! indexed SQLite read. With no traffic it stays in shadow.
//!
//! Training (datasets, adapters, the decision model) is not here; it runs in
//! its own track and its heads register through the same `Head` trait.

use super::backends::{Answer, DecisionBackend, Query};
use crate::db::DbHandle;
use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Instant;

pub const SHADOW: &str = "shadow";
pub const LIVE: &str = "live";
pub const RETIRED: &str = "retired";
pub const STATES: &[&str] = &[SHADOW, LIVE, RETIRED];

/// The decision as a head sees it. `key` is `key_hash` of the decision.
pub struct HeadInput<'a> {
    pub owner: &'a str,
    pub kind: &'a str,
    pub key: &'a str,
    pub ids: &'a [&'a str],
}

/// One head's answer: the chosen option's index and one probability per option.
pub struct HeadAnswer {
    pub choice: usize,
    pub probs: Vec<f64>,
}

pub trait Head: Send + Sync {
    /// Its backend name in attempts and in `decisions.backend` (`head.<name>`).
    fn name(&self) -> &'static str;
    /// The decision kinds it is registered for.
    fn applies(&self, kind: &str) -> bool;
    /// `None` when it has no answer for this decision.
    fn predict(&self, conn: &Connection, input: &HeadInput<'_>) -> rusqlite::Result<Option<HeadAnswer>>;
}

/// Exact-match lookup over past outcomes.
pub struct LookupHead;

impl Head for LookupHead {
    fn name(&self) -> &'static str {
        "head.lookup"
    }
    fn applies(&self, _kind: &str) -> bool {
        true
    }
    fn predict(&self, conn: &Connection, input: &HeadInput<'_>) -> rusqlite::Result<Option<HeadAnswer>> {
        let mut stmt = conn.prepare_cached(
            "SELECT choice, successes, failures FROM decision_lookup WHERE owner = ?1 AND kind = ?2 AND key_hash = ?3",
        )?;
        let rows = stmt.query_map(params![input.owner, input.kind, input.key], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        let mut best: Option<(usize, i64, i64)> = None;
        for row in rows {
            let (choice, s, f) = row?;
            let Some(i) = input.ids.iter().position(|id| *id == choice) else { continue };
            if s > f && best.map_or(true, |(_, bs, bf)| s - f > bs - bf) {
                best = Some((i, s, f));
            }
        }
        Ok(best.map(|(i, s, f)| HeadAnswer { choice: i, probs: lookup_probs(input.ids.len(), i, s, f) }))
    }
}

/// Laplace-smoothed: one success alone gives 2/3, five give 6/7. The rest of
/// the mass is spread evenly over the other options.
pub fn lookup_probs(n: usize, i: usize, s: i64, f: i64) -> Vec<f64> {
    let p = (s as f64 + 1.0) / ((s + f) as f64 + 2.0);
    let rest = if n > 1 { (1.0 - p) / (n - 1) as f64 } else { 0.0 };
    (0..n).map(|j| if j == i { p } else { rest }).collect()
}

static LOOKUP: LookupHead = LookupHead;

/// The registered heads. A trained head from the model track joins here.
pub fn heads() -> Vec<&'static dyn Head> {
    vec![&LOOKUP]
}

fn head_by_name(name: &str) -> Option<&'static dyn Head> {
    heads().into_iter().find(|h| h.name() == name)
}

/// The lookup key of one decision. Option order is part of it: the same
/// labels in another order are another question for the caller's ids.
pub fn key_hash(kind: &str, context_hash: &str, question: Option<&str>, ids: &[&str], texts: &[&str]) -> String {
    let mut h = Sha256::new();
    for part in [kind, context_hash, question.unwrap_or("")] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    for (id, text) in ids.iter().zip(texts) {
        h.update(id.as_bytes());
        h.update([1u8]);
        h.update(text.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

// ── promotion bar ───────────────────────────────────────────────────────────

/// One kind's promotion and demotion bar. Built-in defaults, overridden per
/// kind (or `default`) by the JSON in `ALLTERNIT_DECISIONS_FLYWHEEL`, e.g.
/// `{"default": {"min_samples": 300}, "element": {"promote_at": 0.98}}`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Bar {
    /// Labelled answers needed since the last transition before promotion.
    pub min_samples: u64,
    /// Accuracy against outcomes needed to go live.
    pub promote_at: f64,
    /// Live accuracy under this demotes (hysteresis: below `promote_at`).
    pub demote_at: f64,
    /// Labelled live answers needed before a demotion is judged.
    pub min_demote_samples: u64,
    /// Most recent rows the metrics look at.
    pub window: u64,
    /// Mean confidence may exceed accuracy by at most this much.
    pub max_overconfidence: f64,
    /// Off for kinds a head must never take over by itself (`safety`).
    pub auto_promote: bool,
}

impl Default for Bar {
    fn default() -> Self {
        Self { min_samples: 200, promote_at: 0.97, demote_at: 0.93, min_demote_samples: 50, window: 1000, max_overconfidence: 0.05, auto_promote: true }
    }
}

pub fn bar(kind: &str) -> Bar {
    let over: HashMap<String, Value> = std::env::var("ALLTERNIT_DECISIONS_FLYWHEEL")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut v = serde_json::to_value(Bar::default()).unwrap_or(Value::Null);
    if kind == "safety" {
        v["auto_promote"] = json!(false);
    }
    for key in ["default", kind] {
        if let Some(Value::Object(o)) = over.get(key) {
            for (k, x) in o {
                v[k.as_str()] = x.clone();
            }
        }
    }
    let mut b: Bar = serde_json::from_value(v).unwrap_or_default();
    b.min_samples = b.min_samples.max(1);
    b.window = b.window.max(b.min_samples).max(b.min_demote_samples);
    b.demote_at = b.demote_at.min(b.promote_at);
    b
}

// ── metrics ─────────────────────────────────────────────────────────────────

/// What a decision's outcome says about the right option.
#[derive(Debug, PartialEq)]
pub enum Truth {
    Right(String),
    Wrong(String),
    Unknown,
}

/// `success` confirms the label (or the served choice); `failure` names the
/// right label when it differs from the served choice, otherwise rules the
/// served choice out. `error` and `skipped` say nothing about the choice.
pub fn truth(served: Option<&str>, status: Option<&str>, label: Option<&str>) -> Truth {
    match status {
        Some("success") => label.or(served).map_or(Truth::Unknown, |c| Truth::Right(c.into())),
        Some("failure") => match (label, served) {
            (Some(l), s) if Some(l) != s => Truth::Right(l.into()),
            (_, Some(s)) => Truth::Wrong(s.into()),
            _ => Truth::Unknown,
        },
        _ => Truth::Unknown,
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Metrics {
    /// Decisions the head saw.
    pub decisions: u64,
    /// ... and answered.
    pub answered: u64,
    /// Answered where something was served, and the same choice.
    pub compared: u64,
    pub agreed: u64,
    /// Answered with a known outcome, and right.
    pub labelled: u64,
    pub correct: u64,
    pub coverage: Option<f64>,
    pub agreement: Option<f64>,
    pub accuracy: Option<f64>,
    /// Mean confidence on labelled answers minus accuracy (> 0: overconfident).
    pub overconfidence: Option<f64>,
    pub p50_latency_ms: Option<f64>,
    pub p95_latency_ms: Option<f64>,
}

/// One shadow row joined to its decision: head choice, head confidence,
/// served choice, outcome status, outcome label, head latency.
pub type Sample = (Option<String>, Option<f64>, Option<String>, Option<String>, Option<String>, f64);

pub fn score(rows: &[Sample]) -> Metrics {
    let mut m = Metrics { decisions: rows.len() as u64, ..Default::default() };
    let mut conf_sum = 0.0;
    let mut lat: Vec<f64> = rows.iter().map(|r| r.5).collect();
    for (head, conf, served, status, label) in rows.iter().map(|r| (&r.0, r.1, &r.2, &r.3, &r.4)) {
        let Some(head) = head else { continue };
        m.answered += 1;
        if let Some(s) = served {
            m.compared += 1;
            m.agreed += (s == head) as u64;
        }
        let right = match truth(served.as_deref(), status.as_deref(), label.as_deref()) {
            Truth::Right(t) => Some(&t == head),
            Truth::Wrong(w) if &w == head => Some(false),
            _ => None,
        };
        if let Some(ok) = right {
            m.labelled += 1;
            m.correct += ok as u64;
            conf_sum += conf.unwrap_or(0.0);
        }
    }
    let ratio = |a: u64, b: u64| (b > 0).then(|| a as f64 / b as f64);
    m.coverage = ratio(m.answered, m.decisions);
    m.agreement = ratio(m.agreed, m.compared);
    m.accuracy = ratio(m.correct, m.labelled);
    m.overconfidence = m.accuracy.map(|a| conf_sum / m.labelled as f64 - a);
    lat.sort_by(f64::total_cmp);
    let pct = |p: f64| (!lat.is_empty()).then(|| lat[((lat.len() - 1) as f64 * p).round() as usize]);
    m.p50_latency_ms = pct(0.5);
    m.p95_latency_ms = pct(0.95);
    m
}

pub fn metrics(conn: &Connection, head: &str, kind: &str, since: &str, window: u64) -> rusqlite::Result<Metrics> {
    let mut stmt = conn.prepare_cached(
        "SELECT s.choice, s.confidence, d.choice, d.outcome_status, d.outcome_label, s.latency_ms
         FROM decision_shadow s JOIN decisions d ON d.id = s.decision_id
         WHERE s.head = ?1 AND s.kind = ?2 AND s.created_at >= ?3
         ORDER BY s.created_at DESC LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(params![head, kind, since, window as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<rusqlite::Result<Vec<Sample>>>()?;
    Ok(score(&rows))
}

/// The automatic transition the metrics call for, if any.
pub fn next_state(state: &str, m: &Metrics, b: &Bar) -> Option<(&'static str, String)> {
    let acc = m.accuracy?;
    match state {
        SHADOW if b.auto_promote
            && m.labelled >= b.min_samples
            && acc >= b.promote_at
            && m.overconfidence.unwrap_or(0.0) <= b.max_overconfidence =>
        {
            Some((LIVE, format!("accuracy {acc:.4} ≥ {} over {} labelled", b.promote_at, m.labelled)))
        }
        LIVE if m.labelled >= b.min_demote_samples && acc < b.demote_at => {
            Some((SHADOW, format!("live accuracy {acc:.4} < {} over {} labelled", b.demote_at, m.labelled)))
        }
        _ => None,
    }
}

// ── state ───────────────────────────────────────────────────────────────────

/// The head's state for a kind, registering it in shadow on first sight.
pub fn state(conn: &Connection, head: &str, kind: &str) -> rusqlite::Result<(String, String)> {
    let now = super::super::store::now();
    conn.execute(
        "INSERT OR IGNORE INTO decision_heads (head, kind, state, state_since, updated_at) VALUES (?1, ?2, 'shadow', ?3, ?3)",
        params![head, kind, now],
    )?;
    conn.query_row("SELECT state, state_since FROM decision_heads WHERE head = ?1 AND kind = ?2", params![head, kind], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
}

/// Read-only state lookup for the latency path (`None` = never seen).
pub fn current_state(conn: &Connection, head: &str, kind: &str) -> Option<String> {
    conn.query_row("SELECT state FROM decision_heads WHERE head = ?1 AND kind = ?2", params![head, kind], |r| r.get(0)).ok()
}

pub fn transition(conn: &Connection, head: &str, kind: &str, to: &str, reason: &str, actor: &str, m: &Metrics) -> rusqlite::Result<()> {
    let (from, _) = state(conn, head, kind)?;
    let now = super::super::store::now();
    conn.execute(
        "UPDATE decision_heads SET state = ?3, state_since = ?4, updated_at = ?4 WHERE head = ?1 AND kind = ?2",
        params![head, kind, to, now],
    )?;
    conn.execute(
        "INSERT INTO decision_head_audit (id, head, kind, from_state, to_state, reason, actor, metrics_json, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![super::super::store::new_id("hda"), head, kind, from, to, reason, actor, serde_json::to_string(m).unwrap_or_default(), now],
    )?;
    tracing::info!(head, kind, from = %from, to, reason, actor, "decision head transition");
    Ok(())
}

/// Re-judge every registered head for `kind`; returns the transitions made.
pub fn evaluate(conn: &Connection, kind: &str) -> rusqlite::Result<Vec<(String, String)>> {
    evaluate_with(conn, kind, &bar(kind))
}

pub fn evaluate_with(conn: &Connection, kind: &str, b: &Bar) -> rusqlite::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for h in heads().into_iter().filter(|h| h.applies(kind)) {
        let (st, since) = state(conn, h.name(), kind)?;
        let m = metrics(conn, h.name(), kind, &since, b.window)?;
        if let Some((to, why)) = next_state(&st, &m, b) {
            transition(conn, h.name(), kind, to, &why, "flywheel", &m)?;
            out.push((h.name().to_string(), to.to_string()));
        }
    }
    Ok(out)
}

// ── serving: live heads in the chain ────────────────────────────────────────

/// A live head as a chain backend for one decision.
pub struct LiveHead {
    head: &'static dyn Head,
    db: DbHandle,
    owner: String,
    key: String,
}

#[async_trait]
impl DecisionBackend for LiveHead {
    fn name(&self) -> &'static str {
        self.head.name()
    }
    fn vision(&self) -> bool {
        // The key covers the context hash, which does not cover the image; a
        // screenshot decision is not the same decision twice.
        false
    }
    fn enabled(&self) -> bool {
        true
    }
    async fn decide(&self, q: &Query<'_>) -> Result<Answer, String> {
        let input = HeadInput { owner: &self.owner, kind: q.kind, key: &self.key, ids: &q.ids };
        let predicted = with_head_conn(&self.db, |conn| self.head.predict(conn, &input)).map_err(|e| e.to_string())?;
        match predicted {
            Some(a) => Ok(Answer { probs: a.probs, abstain: false, detail: json!({ "head": self.head.name() }) }),
            None => Err("no match".into()),
        }
    }
}

thread_local! {
    /// One SQLite connection per worker thread for live heads: a head answers
    /// in well under 10 ms only when it skips the open (and keeps its
    /// prepared statement cache) on every decision.
    static HEAD_CONN: std::cell::RefCell<Option<(std::path::PathBuf, Connection)>> = const { std::cell::RefCell::new(None) };
}

fn with_head_conn<T>(db: &DbHandle, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    HEAD_CONN.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().map(|(p, _)| p.as_path()) != Some(db.path()) {
            *slot = Some((db.path().to_path_buf(), db.connect()?));
        }
        let (_, conn) = slot.as_ref().expect("set above");
        f(conn)
    })
}

/// The `head` placeholder backend: never runs by itself. `expand` swaps it
/// for the kind's live heads.
pub struct HeadSlot;

#[async_trait]
impl DecisionBackend for HeadSlot {
    fn name(&self) -> &'static str {
        "head"
    }
    fn vision(&self) -> bool {
        false
    }
    fn enabled(&self) -> bool {
        false
    }
    async fn decide(&self, _q: &Query<'_>) -> Result<Answer, String> {
        Err("head slot".into())
    }
}

/// Replace the `head` slot with this kind's live heads (none → removed).
pub fn expand(chain: Vec<Box<dyn DecisionBackend>>, db: &DbHandle, owner: &str, kind: &str, key: &str) -> Vec<Box<dyn DecisionBackend>> {
    if !chain.iter().any(|b| b.name() == "head") {
        return chain;
    }
    let live: Vec<&'static dyn Head> = db
        .connect()
        .ok()
        .map(|c| heads().into_iter().filter(|h| h.applies(kind) && current_state(&c, h.name(), kind).as_deref() == Some(LIVE)).collect())
        .unwrap_or_default();
    let mut out: Vec<Box<dyn DecisionBackend>> = Vec::with_capacity(chain.len() + live.len());
    for b in chain {
        if b.name() == "head" {
            for h in &live {
                out.push(Box::new(LiveHead { head: *h, db: db.clone(), owner: owner.into(), key: key.into() }));
            }
        } else {
            out.push(b);
        }
    }
    out
}

// ── shadow recording and learning ───────────────────────────────────────────

/// Owned copy of a stored decision for the shadow pass.
pub struct Shadow {
    pub decision_id: String,
    pub owner: String,
    pub kind: String,
    pub key: String,
    pub ids: Vec<String>,
    pub has_image: bool,
}

/// Run every registered, non-retired head for the decision's kind and record
/// its answer. Runs after the decision is stored, never on the latency path.
/// Store the decision's lookup key. Synchronous, right after the decision
/// row, so an outcome that arrives before the shadow pass still finds it.
pub fn record_key(db: &DbHandle, s: &Shadow) -> rusqlite::Result<()> {
    db.connect()?.execute(
        "INSERT OR IGNORE INTO decision_keys (decision_id, owner, kind, key_hash) VALUES (?1, ?2, ?3, ?4)",
        params![s.decision_id, s.owner, s.kind, s.key],
    )?;
    Ok(())
}

pub fn record_shadow(db: &DbHandle, s: &Shadow) -> rusqlite::Result<()> {
    let conn = db.connect()?;
    // Its outcome is already learned: a prediction now would see its own
    // answer. Not counted.
    let learned: bool = conn
        .query_row("SELECT learned FROM decision_keys WHERE decision_id = ?1", params![s.decision_id], |r| r.get(0))
        .optional()?
        .unwrap_or(false);
    if learned {
        return Ok(());
    }
    let ids: Vec<&str> = s.ids.iter().map(String::as_str).collect();
    let input = HeadInput { owner: &s.owner, kind: &s.kind, key: &s.key, ids: &ids };
    for h in heads().into_iter().filter(|h| h.applies(&s.kind)) {
        let (st, _) = state(&conn, h.name(), &s.kind)?;
        if st == RETIRED {
            continue;
        }
        // After `state`, so a first row is never older than its head's state_since.
        let now = super::super::store::now();
        let t = Instant::now();
        // Heads are blind to images (see LiveHead::vision): recorded as no answer.
        let a = if s.has_image { None } else { h.predict(&conn, &input)? };
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        conn.execute(
            "INSERT OR REPLACE INTO decision_shadow (decision_id, head, kind, head_state, choice, confidence, latency_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                s.decision_id,
                h.name(),
                s.kind,
                st,
                a.as_ref().map(|a| ids[a.choice]),
                a.as_ref().map(|a| a.probs[a.choice]),
                ms,
                now
            ],
        )?;
    }
    Ok(())
}

/// An outcome arrived: fold it into the lookup table (once per decision),
/// then re-judge the kind's heads.
pub fn on_outcome(db: &DbHandle, decision_id: &str, owner: &str) -> rusqlite::Result<Vec<(String, String)>> {
    let conn = db.connect()?;
    let row: Option<(String, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT kind, choice, outcome_status, outcome_label FROM decisions WHERE id = ?1 AND owner = ?2",
            params![decision_id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((kind, served, status, label)) = row else { return Ok(vec![]) };
    let key: Option<(String, bool)> = conn
        .query_row("SELECT key_hash, learned FROM decision_keys WHERE decision_id = ?1 AND owner = ?2", params![decision_id, owner], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    if let Some((key, false)) = key {
        let (choice, col) = match truth(served.as_deref(), status.as_deref(), label.as_deref()) {
            Truth::Right(c) => (Some(c), "successes"),
            Truth::Wrong(c) => (Some(c), "failures"),
            Truth::Unknown => (None, ""),
        };
        if let Some(choice) = choice {
            conn.execute(
                &format!(
                    "INSERT INTO decision_lookup (owner, kind, key_hash, choice, {col}, updated_at) VALUES (?1, ?2, ?3, ?4, 1, ?5)
                     ON CONFLICT(owner, kind, key_hash, choice) DO UPDATE SET {col} = {col} + 1, updated_at = excluded.updated_at"
                ),
                params![owner, kind, key, choice, super::super::store::now()],
            )?;
            conn.execute("UPDATE decision_keys SET learned = 1 WHERE decision_id = ?1", params![decision_id])?;
        }
    }
    evaluate(&conn, &kind)
}

// ── read and admin endpoints ────────────────────────────────────────────────

use super::super::{ApiError, ApiResult, RequestId};
use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Extension, Json,
};
use std::sync::Arc;

/// Every head × kind with its state, bar, metrics since the last transition
/// and its recent audit trail. Aggregates only: no context, no options.
pub fn list(conn: &Connection) -> rusqlite::Result<Value> {
    let mut stmt = conn.prepare("SELECT head, kind, state, state_since, updated_at FROM decision_heads ORDER BY kind, head")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut data = Vec::new();
    for (head, kind, st, since, updated) in rows {
        let b = bar(&kind);
        let m = metrics(conn, &head, &kind, &since, b.window)?;
        let mut a = conn.prepare_cached(
            "SELECT from_state, to_state, reason, actor, at FROM decision_head_audit WHERE head = ?1 AND kind = ?2 ORDER BY at DESC LIMIT 10",
        )?;
        let audit = a
            .query_map(params![head, kind], |r| {
                Ok(json!({ "from": r.get::<_, String>(0)?, "to": r.get::<_, String>(1)?, "reason": r.get::<_, String>(2)?,
                           "actor": r.get::<_, String>(3)?, "at": r.get::<_, String>(4)? }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        data.push(json!({
            "object": "decision_head", "head": head, "kind": kind, "state": st, "state_since": since, "updated_at": updated,
            "metrics": m, "bar": b, "next": next_state(&st, &m, &b).map(|(to, why)| json!({ "state": to, "reason": why })),
            "audit": audit,
        }));
    }
    Ok(json!({ "object": "list", "data": data, "registered": heads().iter().map(|h| h.name()).collect::<Vec<_>>() }))
}

pub async fn list_heads(State(st): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>, Extension(rid): Extension<RequestId>) -> ApiResult {
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    list(&conn).map(|v| Json(v).into_response()).map_err(|e| ApiError::internal(e, &rid))
}

/// Head state is server-wide, so only the people named in
/// `ALLTERNIT_DECISIONS_ADMINS` (user ids, comma separated; `*` = anyone,
/// for a single-user Desktop) may move it by hand.
pub fn is_flywheel_admin(user_id: &str) -> bool {
    std::env::var("ALLTERNIT_DECISIONS_ADMINS")
        .map(|v| v.split(',').map(str::trim).any(|u| u == "*" || u == user_id))
        .unwrap_or(false)
}

#[derive(serde::Deserialize)]
pub struct StateIn {
    state: String,
    #[serde(default)]
    reason: Option<String>,
}

pub async fn put_head(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path((head, kind)): Path<(String, String)>,
    Json(body): Json<StateIn>,
) -> ApiResult {
    if !is_flywheel_admin(&user.user_id) {
        return Err(ApiError::new(403, "PERMISSION", "ERR_PERMISSION_DENIED", "only a decisions admin can change a head's state", &rid));
    }
    let Some(h) = head_by_name(&head) else { return Err(ApiError::not_found("decision head", &rid)) };
    if !STATES.contains(&body.state.as_str()) || !h.applies(&kind) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", format!("state must be one of {}", STATES.join(", ")), &rid)
            .param(Some("state".into())));
    }
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    let (_, since) = state(&conn, &head, &kind).map_err(|e| ApiError::internal(e, &rid))?;
    let m = metrics(&conn, &head, &kind, &since, bar(&kind).window).map_err(|e| ApiError::internal(e, &rid))?;
    let reason = body.reason.unwrap_or_else(|| "manual".into()).chars().take(500).collect::<String>();
    transition(&conn, &head, &kind, &body.state, &reason, &user.user_id, &m).map_err(|e| ApiError::internal(e, &rid))?;
    list(&conn).map(|v| Json(v).into_response()).map_err(|e| ApiError::internal(e, &rid))
}
