//! Replay acceptance for decision-head promotion (B5).
//!
//! A shadow head that clears the live bar (`flywheel::next_state`) is also
//! held to the **replay evidence** for its kind when a replay set exists:
//! the latest batch replay (`POST /v1/replays/batch` with
//! `overrides.decision_heads`, run by a decisions admin) that re-decided the
//! labelled decisions of recent finished runs with this head. It passes when
//!
//! - the replay set has labelled decisions;
//! - accuracy ≥ baseline − ε (`Bar::replay_epsilon`, default 0.01), where the
//!   baseline is the served choice on the same labelled decisions and the
//!   candidate's accuracy is the system's with the head live (its confident
//!   answer, else the chain's served answer);
//! - and cost **or** latency improves on the baseline (strictly lower mean).
//!
//! pass@1 and pass@k (`Bar::replay_k`, default 5) are reported with the
//! verdict. A nondeterministic head is sampled k times per decision; a
//! deterministic head (every registered head today) is sampled once and its
//! pass@k equals its pass@1. No replay set: promotion uses the live bar alone.
//!
//! The evidence id lands in the promotion's audit row
//! (`decision_head_audit.metrics_json.replay_evidence_id`).

use super::super::{ApiError, ApiResult, RequestId};
use super::flywheel::{self, Bar};
use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Extension, Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Float slack for ≥ / < comparisons of rates computed from counts.
const SLACK: f64 = 1e-9;

/// One head × kind result of a batch replay (`decision_replay_evidence`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Evidence {
    /// The batch replay id (`rpb_…`).
    pub id: String,
    pub head: String,
    pub kind: String,
    pub head_version: String,
    pub owner: String,
    pub created_at: String,
    pub runs: u64,
    pub decisions: u64,
    pub labelled: u64,
    pub accuracy: Option<f64>,
    pub baseline_accuracy: Option<f64>,
    pub cost_usd: Option<f64>,
    pub baseline_cost_usd: Option<f64>,
    pub latency_ms: Option<f64>,
    pub baseline_latency_ms: Option<f64>,
    pub pass_at_1: Option<f64>,
    pub pass_at_k: Option<f64>,
    pub k: u32,
    pub deterministic: bool,
    #[serde(skip)]
    pub metrics: Value,
}

pub fn record(conn: &Connection, e: &Evidence) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO decision_replay_evidence (id, head, kind, head_version, owner, created_at, runs, decisions, labelled,
            accuracy, baseline_accuracy, cost_usd, baseline_cost_usd, latency_ms, baseline_latency_ms, pass_at_1, pass_at_k, k,
            deterministic, metrics_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            e.id, e.head, e.kind, e.head_version, e.owner, e.created_at, e.runs as i64, e.decisions as i64, e.labelled as i64,
            e.accuracy, e.baseline_accuracy, e.cost_usd, e.baseline_cost_usd, e.latency_ms, e.baseline_latency_ms, e.pass_at_1,
            e.pass_at_k, e.k as i64, e.deterministic, e.metrics.to_string()
        ],
    )?;
    Ok(())
}

/// The latest replay evidence for a head and kind, if a replay set exists.
pub fn latest(conn: &Connection, head: &str, kind: &str) -> rusqlite::Result<Option<Evidence>> {
    conn.query_row(
        "SELECT id, head, kind, head_version, owner, created_at, runs, decisions, labelled, accuracy, baseline_accuracy,
            cost_usd, baseline_cost_usd, latency_ms, baseline_latency_ms, pass_at_1, pass_at_k, k, deterministic, metrics_json
         FROM decision_replay_evidence WHERE head = ?1 AND kind = ?2 ORDER BY created_at DESC, rowid DESC LIMIT 1",
        params![head, kind],
        |r| {
            Ok(Evidence {
                id: r.get(0)?,
                head: r.get(1)?,
                kind: r.get(2)?,
                head_version: r.get(3)?,
                owner: r.get(4)?,
                created_at: r.get(5)?,
                runs: r.get::<_, i64>(6)?.max(0) as u64,
                decisions: r.get::<_, i64>(7)?.max(0) as u64,
                labelled: r.get::<_, i64>(8)?.max(0) as u64,
                accuracy: r.get(9)?,
                baseline_accuracy: r.get(10)?,
                cost_usd: r.get(11)?,
                baseline_cost_usd: r.get(12)?,
                latency_ms: r.get(13)?,
                baseline_latency_ms: r.get(14)?,
                pass_at_1: r.get(15)?,
                pass_at_k: r.get(16)?,
                k: r.get::<_, i64>(17)?.clamp(1, 1000) as u32,
                deterministic: r.get(18)?,
                metrics: serde_json::from_str(&r.get::<_, String>(19)?).unwrap_or(Value::Null),
            })
        },
    )
    .optional()
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Acceptance {
    pub passed: bool,
    pub evidence_id: String,
    pub epsilon: f64,
    pub k: u32,
    pub pass_at_1: Option<f64>,
    pub pass_at_k: Option<f64>,
    pub checks: Vec<Check>,
}

impl Acceptance {
    /// One line: the failed checks, or every check when it passed.
    pub fn summary(&self) -> String {
        let pick: Vec<&Check> = if self.passed { self.checks.iter().collect() } else { self.checks.iter().filter(|c| !c.passed).collect() };
        pick.iter().map(|c| c.detail.as_str()).collect::<Vec<_>>().join("; ")
    }
}

fn fmt(x: Option<f64>) -> String {
    x.map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".into())
}

/// The acceptance rule over one piece of replay evidence.
pub fn judge(e: &Evidence, b: &Bar) -> Acceptance {
    let eps = b.replay_epsilon;
    let mut checks = vec![Check {
        name: "labelled",
        passed: e.labelled > 0,
        detail: format!("{} labelled decisions in replay {}", e.labelled, e.id),
    }];
    let acc_ok = matches!((e.accuracy, e.baseline_accuracy), (Some(a), Some(base)) if a + SLACK >= base - eps);
    checks.push(Check {
        name: "accuracy",
        passed: acc_ok,
        detail: format!("accuracy {} {} baseline {} - ε {eps}", fmt(e.accuracy), if acc_ok { "≥" } else { "<" }, fmt(e.baseline_accuracy)),
    });
    let lower = |a: Option<f64>, base: Option<f64>| matches!((a, base), (Some(a), Some(base)) if a < base - SLACK);
    let cost_better = lower(e.cost_usd, e.baseline_cost_usd);
    let latency_better = lower(e.latency_ms, e.baseline_latency_ms);
    checks.push(Check {
        name: "cost_or_latency",
        passed: cost_better || latency_better,
        detail: format!(
            "cost {} vs baseline {} ({}), latency {} ms vs baseline {} ms ({})",
            fmt(e.cost_usd),
            fmt(e.baseline_cost_usd),
            if cost_better { "better" } else { "not better" },
            fmt(e.latency_ms),
            fmt(e.baseline_latency_ms),
            if latency_better { "better" } else { "not better" }
        ),
    });
    Acceptance {
        passed: checks.iter().all(|c| c.passed),
        evidence_id: e.id.clone(),
        epsilon: eps,
        k: e.k,
        pass_at_1: e.pass_at_1,
        pass_at_k: e.pass_at_k,
        checks,
    }
}

/// pass@1 and pass@k over labelled decisions, each sampled `n` times with
/// `c` correct samples (unbiased estimator `1 - C(n-c, k) / C(n, k)`, Chen et
/// al. 2021). With a single sample per decision (deterministic heads),
/// pass@k is pass@1 by definition.
pub fn pass_at(samples: &[(u32, u32)], k: u32) -> (Option<f64>, Option<f64>) {
    let rows: Vec<(u32, u32)> = samples.iter().copied().filter(|(n, _)| *n > 0).collect();
    if rows.is_empty() {
        return (None, None);
    }
    let len = rows.len() as f64;
    let p1 = rows.iter().map(|(n, c)| *c.min(n) as f64 / *n as f64).sum::<f64>() / len;
    if rows.iter().all(|(n, _)| *n == 1) {
        return (Some(p1), Some(p1));
    }
    let pk = rows
        .iter()
        .map(|&(n, c)| {
            let c = c.min(n);
            let k = k.min(n);
            if n - c < k {
                return 1.0;
            }
            // C(n-c, k) / C(n, k) = Π_{i=0}^{k-1} (n-c-i) / (n-i)
            1.0 - (0..k).map(|i| (n - c - i) as f64 / (n - i) as f64).product::<f64>()
        })
        .sum::<f64>()
        / len;
    (Some(p1), Some(pk))
}

/// Everything the admin check reports for one head and kind.
pub fn explain(conn: &Connection, head: &str, kind: &str) -> rusqlite::Result<Value> {
    explain_with(conn, head, kind, &flywheel::bar(kind))
}

pub fn explain_with(conn: &Connection, head: &str, kind: &str, b: &Bar) -> rusqlite::Result<Value> {
    let b = b.clone();
    let (st, since) = flywheel::state(conn, head, kind)?;
    let m = flywheel::metrics(conn, head, kind, &since, b.window)?;
    let live = flywheel::next_state(&st, &m, &b);
    let live_bar = json!({
        "passed": live.as_ref().is_some_and(|(to, _)| *to == flywheel::LIVE),
        "detail": match (&live, st.as_str()) {
            (Some((to, why)), _) if *to == flywheel::LIVE => why.clone(),
            (_, flywheel::SHADOW) if !b.auto_promote => format!("auto-promotion is off for kind {kind}"),
            (_, flywheel::SHADOW) => format!(
                "needs ≥ {} labelled at accuracy ≥ {} (have {} at {})", b.min_samples, b.promote_at, m.labelled, fmt(m.accuracy)
            ),
            _ => format!("head is {st}; only a shadow head is promoted"),
        },
    });
    let evidence = latest(conn, head, kind)?;
    let verdict = evidence.as_ref().map(|e| judge(e, &b));
    let replay_ok = verdict.as_ref().map_or(true, |v| v.passed);
    let would = st == flywheel::SHADOW && live_bar["passed"] == true && replay_ok;
    let why = match (&verdict, live_bar["passed"] == true) {
        (_, false) => format!("live bar not met: {}", live_bar["detail"].as_str().unwrap_or_default()),
        (Some(v), true) if !v.passed => format!("replay evidence {} failed: {}", v.evidence_id, v.summary()),
        (Some(v), true) => format!("live bar met and replay evidence {} passed", v.evidence_id),
        (None, true) => "live bar met; no replay set for this kind, so the live bar alone decides".into(),
    };
    Ok(json!({
        "object": "decision_head_acceptance",
        "head": head,
        "kind": kind,
        "state": st,
        "passed": would,
        "reason": why,
        "live_bar": live_bar,
        "metrics": m,
        "bar": b,
        "replay": verdict.map(|v| json!({ "evidence": evidence, "acceptance": v })),
    }))
}

/// `POST /v1/decisions/heads/:head/:kind/acceptance` (decisions admins):
/// run the promotion check for a head and kind and say why it passes or
/// fails. Read-only: state moves stay with the flywheel and `PUT`.
pub async fn check_head(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path((head, kind)): Path<(String, String)>,
) -> ApiResult {
    if !flywheel::is_flywheel_admin(&user.user_id) {
        return Err(ApiError::new(403, "PERMISSION", "ERR_PERMISSION_DENIED", "only a decisions admin can run the acceptance check", &rid));
    }
    let Some(h) = flywheel::head_by_name(&head) else { return Err(ApiError::not_found("decision head", &rid)) };
    if !h.applies(&kind) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", format!("{head} does not apply to kind {kind}"), &rid).param(Some("kind".into())));
    }
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    explain(&conn, &head, &kind).map(|v| Json(v).into_response()).map_err(|e| ApiError::internal(e, &rid))
}
