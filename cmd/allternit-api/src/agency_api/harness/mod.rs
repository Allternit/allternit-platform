//! C6: harness learning. Workflows that improve themselves, human-approved.
//!
//! A decisions admin asks for proposals for a task type
//! (`POST /v1/harness/proposals`). The server
//! 1. gathers **evidence**: recent finished runs of the type and their
//!    trace-monitor findings, the admin's recent eval runs, Decision
//!    Runtime metrics (descent) for the graph's S1 decision kinds, and the
//!    baseline evaluation's own failures and replay divergences;
//! 2. produces candidate graph edits: deterministic **mutation operators**
//!    ([`mutate`]: valid by construction) and, optionally, one **LLM
//!    proposal** (the oracle/planner model through gizzi-code, tools off,
//!    asked for RFC 6902 patches). Every candidate is validated through
//!    `task_types::bind` (the Factory validator) and the structural
//!    guardrails; invalid ones are discarded with the reason. At most `k`
//!    (default 4) are evaluated;
//! 3. **evaluates** baseline and every candidate on the same cases: the
//!    task type's shipped eval set and the requested suites (scripted, on
//!    the candidate graph), and recorded-only replays of recent real runs
//!    with a `graph_version` inline-candidate override. Pass-rate delta
//!    with a paired bootstrap interval, cost and latency deltas, new
//!    high-stakes failures;
//! 4. applies the **acceptance rule** (VERA-style, [`Rule`]) and stores
//!    every evaluated candidate as a proposal: `pending` when it passed,
//!    `failed_acceptance` otherwise, each with its reasons, evidence and a
//!    readable diff.
//!
//! **Human approval.** Pending proposals are an admin-facing list
//! (`GET /v1/harness/proposals?status=pending`), not run attention: an
//! attention request belongs to a run (it parks the run and resolves into
//! it), and a proposal has no run. Approving registers the candidate as a
//! new graph **version** of the task type (`<stem>.v<N>`) in the version
//! registry (a ledger object; the shipped JSON is never edited). New runs
//! pin the active version into their TaskIR at creation
//! ([`pin_active_version`]), so a run keeps the graph it started with;
//! `POST /v1/harness/versions/:task_type/rollback` returns new runs to the
//! previous version. Every transition is audited.
//!
//! No GPU and no training here: the proposer is an existing model or a
//! deterministic operator, and the "learning" is search plus selection
//! under evals.

pub mod mutate;
#[cfg(test)]
mod tests;

use super::decisions::flywheel;
use super::evals::{self, graders, stats, Case, PassRule, Source, Suite};
use super::replays::{self, Fail, GraphOverride, Overrides};
use super::store::{
    new_id, now, AgencyStore, EV_EVAL_RUN_STATE, EV_EVAL_SUITE_STATE, EV_HARNESS_AUDIT_STATE, EV_HARNESS_PROPOSAL_STATE, EV_HARNESS_SEARCH_STATE,
    EV_HARNESS_VERSIONS_STATE, SYSTEM_OWNER,
};
use super::task_types::{self, s1, TaskType};
use super::{executor, guard, monitor, page, store, ApiError, ApiResult, RequestId};
use crate::auth::AuthUser;
use crate::AppState;
use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use mutate::Candidate;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub const DEFAULT_K: usize = 4;
pub const MAX_K: usize = 8;
pub const DEFAULT_REPLAY_RUNS: usize = 10;
pub const MAX_REPLAY_RUNS: usize = 30;
const MAX_SUITES: usize = 5;
const EVIDENCE_RUNS: usize = 50;
const EVIDENCE_EVAL_RUNS: usize = 5;
const EVIDENCE_PROMPT_BYTES: usize = 8 * 1024;

// ── acceptance rule ─────────────────────────────────────────────────────────

/// How high-stakes cases gate a candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HighStakes {
    /// Reject when a high-stakes case that passed on the baseline fails (default).
    NoNew,
    /// Reject when any high-stakes case fails on the candidate.
    None,
}

/// VERA-style acceptance: a candidate passes only if
/// - no high-stakes failure (per [`HighStakes`]), and
/// - pass rate >= baseline + `min_gain`, OR (pass rate >= baseline - `epsilon`
///   AND cost or latency improves by >= `efficiency_gain`), and
/// - the lower bound of the paired 95% interval of the delta > -`epsilon`.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub min_gain: f64,
    pub epsilon: f64,
    pub efficiency_gain: f64,
    pub high_stakes: HighStakes,
}

impl Default for Rule {
    fn default() -> Self {
        Self { min_gain: 0.05, epsilon: 0.02, efficiency_gain: 0.10, high_stakes: HighStakes::NoNew }
    }
}

impl Rule {
    pub fn parse(v: Option<&Value>) -> Result<Self, Fail> {
        let mut r = Self::default();
        let o = match v {
            None | Some(Value::Null) => return Ok(r),
            Some(Value::Object(o)) => o,
            Some(_) => return Err(Fail::input("acceptance", "acceptance must be an object")),
        };
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "min_gain" | "epsilon" | "efficiency_gain" | "high_stakes")) {
            return Err(Fail::unsupported(&format!("acceptance.{k}"), format!("unknown field `{k}`")));
        }
        let num = |k: &str, lo: f64, hi: f64, d: f64| -> Result<f64, Fail> {
            match o.get(k) {
                None | Some(Value::Null) => Ok(d),
                Some(v) => v.as_f64().filter(|x| (lo..=hi).contains(x)).ok_or_else(|| Fail::input(&format!("acceptance.{k}"), format!("{k} must be {lo}-{hi}"))),
            }
        };
        r.min_gain = num("min_gain", 0.0, 0.5, r.min_gain)?;
        r.epsilon = num("epsilon", 0.0, 0.2, r.epsilon)?;
        r.efficiency_gain = num("efficiency_gain", 0.01, 0.9, r.efficiency_gain)?;
        r.high_stakes = match o.get("high_stakes").and_then(Value::as_str) {
            None => r.high_stakes,
            Some("no_new") => HighStakes::NoNew,
            Some("none") => HighStakes::None,
            Some(_) => return Err(Fail::input("acceptance.high_stakes", "high_stakes is no_new or none")),
        };
        Ok(r)
    }
    pub fn json(&self) -> Value {
        json!({ "min_gain": self.min_gain, "epsilon": self.epsilon, "efficiency_gain": self.efficiency_gain,
                "high_stakes": match self.high_stakes { HighStakes::NoNew => "no_new", HighStakes::None => "none" } })
    }
}

/// One side (baseline or candidate) of an evaluation.
#[derive(Debug, Clone, Default)]
pub struct Arm {
    /// case key -> passed (graded cases only).
    pub outcomes: BTreeMap<String, bool>,
    pub high_stakes: BTreeSet<String>,
    pub skipped: Vec<Value>,
    pub cases: Vec<Value>,
    pub cost_usd: f64,
    pub cost_units: f64,
    pub latency_ms: f64,
    pub latency_units: f64,
    /// Replay divergences by node.
    pub divergences: BTreeMap<String, u64>,
    /// Failing cases' stop nodes.
    pub failed_at: BTreeMap<String, u64>,
}

impl Arm {
    pub fn pass_rate(&self) -> Option<f64> {
        let v: Vec<bool> = self.outcomes.values().copied().collect();
        stats::rate(&v)
    }
    pub fn high_stakes_failures(&self) -> Vec<String> {
        self.high_stakes.iter().filter(|k| self.outcomes.get(*k) == Some(&false)).cloned().collect()
    }
    pub fn summary(&self) -> Value {
        json!({ "graded": self.outcomes.len(), "passed": self.outcomes.values().filter(|x| **x).count(),
                "pass_rate": self.pass_rate().map(stats::round4), "high_stakes_failures": self.high_stakes_failures(),
                "skipped": self.skipped, "cost_usd": stats::round4(self.cost_usd), "cost_units": self.cost_units,
                "latency_ms": stats::round4(self.latency_ms), "latency_units": self.latency_units,
                "failed_at": self.failed_at, "divergences_by_node": self.divergences })
    }
}

/// Relative change of the cost (or latency) metric: USD / ms when the
/// baseline has any (replayed real runs), else the scripted units.
fn rel_change(base_real: f64, cand_real: f64, base_units: f64, cand_units: f64) -> (Option<f64>, &'static str) {
    if base_real > 0.0 {
        (Some((cand_real - base_real) / base_real), "measured")
    } else if base_units > 0.0 {
        (Some((cand_units - base_units) / base_units), "units")
    } else {
        (None, "none")
    }
}

/// Apply `rule` to `base` vs `cand` over the cases graded on both sides.
pub fn decide(rule: &Rule, base: &Arm, cand: &Arm, seed: u64, resamples: usize) -> Value {
    let keys: Vec<&String> = base.outcomes.keys().filter(|k| cand.outcomes.contains_key(*k)).collect();
    let b: Vec<bool> = keys.iter().map(|k| base.outcomes[*k]).collect();
    let c: Vec<bool> = keys.iter().map(|k| cand.outcomes[*k]).collect();
    let mut reasons = vec![];
    if keys.is_empty() {
        return json!({ "passed": false, "reasons": ["no case was graded on both the baseline and the candidate"], "paired_cases": 0 });
    }
    let (br, cr) = (stats::rate(&b).unwrap_or(0.0), stats::rate(&c).unwrap_or(0.0));
    let delta = cr - br;
    let ci = stats::paired_delta_ci95(&b, &c, seed, resamples);
    let (cost, cost_basis) = rel_change(base.cost_usd, cand.cost_usd, base.cost_units, cand.cost_units);
    let (lat, lat_basis) = rel_change(base.latency_ms, cand.latency_ms, base.latency_units, cand.latency_units);
    let hs: Vec<String> = match rule.high_stakes {
        HighStakes::NoNew => keys.iter().filter(|k| cand.high_stakes.contains(**k) && base.outcomes[**k] && !cand.outcomes[**k]).map(|k| k.to_string()).collect(),
        HighStakes::None => keys.iter().filter(|k| cand.high_stakes.contains(**k) && !cand.outcomes[**k]).map(|k| k.to_string()).collect(),
    };
    const TOL: f64 = 1e-9;
    let mut passed = true;
    if !hs.is_empty() {
        passed = false;
        reasons.push(format!("high-stakes failure: {}", hs.join(", ")));
    }
    let gain = delta + TOL >= rule.min_gain;
    let efficient = [cost, lat].iter().flatten().any(|x| *x <= -rule.efficiency_gain + TOL);
    let holds = delta + TOL >= -rule.epsilon;
    if gain {
        reasons.push(format!("pass rate {:.1}% -> {:.1}% (delta {:+.1} points >= +{:.1} required)", br * 100.0, cr * 100.0, delta * 100.0, rule.min_gain * 100.0));
    } else if holds && efficient {
        let pct = |x: Option<f64>| x.map(|v| format!("{:+.1}%", v * 100.0)).unwrap_or_else(|| "n/a".into());
        reasons.push(format!("pass rate held ({:+.1} points, >= -{:.1}) and efficiency improved (cost {}, latency {}; >= {:.0}% required)",
            delta * 100.0, rule.epsilon * 100.0, pct(cost), pct(lat), rule.efficiency_gain * 100.0));
    } else {
        passed = false;
        if !holds {
            reasons.push(format!("pass rate regressed {:.1}% -> {:.1}% ({:+.1} points, worse than -{:.1})", br * 100.0, cr * 100.0, delta * 100.0, rule.epsilon * 100.0));
        } else {
            reasons.push(format!("not enough gain: {:+.1} points (needs +{:.1}) and no cost or latency improvement of {:.0}%",
                delta * 100.0, rule.min_gain * 100.0, rule.efficiency_gain * 100.0));
        }
    }
    match ci {
        Some((lo, _)) if lo + TOL <= -rule.epsilon => {
            passed = false;
            reasons.push(format!("delta interval lower bound {:+.1} points is not above -{:.1}", lo * 100.0, rule.epsilon * 100.0));
        }
        _ => {}
    }
    json!({
        "passed": passed, "reasons": reasons, "paired_cases": keys.len(),
        "baseline_pass_rate": stats::round4(br), "candidate_pass_rate": stats::round4(cr), "delta": stats::round4(delta),
        "delta_ci95": ci.map(|(l, h)| json!({ "low": l, "high": h, "method": "paired_percentile_bootstrap", "seed": seed })),
        "cost_change": cost.map(stats::round4), "cost_basis": cost_basis, "latency_change": lat.map(stats::round4), "latency_basis": lat_basis,
        "new_high_stakes_failures": hs, "rule": rule.json(),
    })
}

// ── evaluation ──────────────────────────────────────────────────────────────

/// Where an evaluation runs: offline (scripted cases only) or on a server
/// (recorded runs are replayed too).
pub enum Env<'a> {
    Offline,
    Online { st: &'a Arc<AppState> },
}

const COST_UNITS: &[(&str, f64)] = &[("S0", 0.0), ("S1", 1.0), ("S2", 10.0), ("S3", 30.0)];
const LATENCY_UNITS: &[(&str, f64)] = &[("S0", 1.0), ("S1", 2.0), ("S2", 10.0), ("S3", 20.0)];

fn units(graph: &Value, node: &str, table: &[(&str, f64)]) -> f64 {
    let role = graph["nodes"].as_array().and_then(|a| a.iter().find(|n| n["node_id"] == node)).and_then(|n| n["cognitive_role"].as_str()).unwrap_or("S0");
    table.iter().find(|(r, _)| *r == role).map(|(_, u)| *u).unwrap_or(0.0)
}

fn add_trajectory(arm: &mut Arm, graph: &Value, t: &graders::Trajectory) {
    for s in &t.steps {
        arm.cost_units += units(graph, &s.node, COST_UNITS);
        arm.latency_units += units(graph, &s.node, LATENCY_UNITS);
    }
    arm.cost_usd += t.cost_usd;
    arm.latency_ms += t.latency_ms;
}

/// Grade one case against `graph` (unbound graph JSON of task type `t`).
/// `Err` only for a reached spending cap (fail closed).
async fn eval_case(env: &Env<'_>, t: &TaskType, graph: &Value, key: &str, case: &Case, arm: &mut Arm) -> Result<(), Fail> {
    let gs = evals::graders_of(case);
    let mut attempts = vec![];
    let mut passes = 0;
    let mut via = "scripted";
    let mut stopped: Option<String> = None;
    let scripted = |f: &evals::Fixture, arm: &mut Arm, attempts: &mut Vec<Value>, passes: &mut u32, stopped: &mut Option<String>| {
        for i in 0..case.k {
            let tr = evals::scripted_trajectory_on(f, Some(graph));
            let (pass, grades) = graders::grade_all(&gs, &tr);
            *passes += pass as u32;
            if !pass {
                *stopped = tr.stopped_at.clone();
            }
            add_trajectory(arm, graph, &tr);
            attempts.push(evals::attempt_json(i, pass, &grades));
        }
    };
    match (&case.source, env) {
        (Source::Scripted(f), _) => scripted(f, arm, &mut attempts, &mut passes, &mut stopped),
        (Source::Run { fixture, .. }, Env::Offline) => match fixture {
            Some(f) => { via = "scripted_fixture"; scripted(f, arm, &mut attempts, &mut passes, &mut stopped) }
            None => {
                arm.skipped.push(json!({ "case": key, "reason": "offline: a replayed case needs the recorded run" }));
                return Ok(());
            }
        },
        (Source::Run { run_id, fixture }, Env::Online { st }) => {
            let src = store(st).load_run(run_id).await.ok().flatten()
                .filter(|r| replays::FINISHED.contains(&r.run["status"].as_str().unwrap_or_default()))
                .filter(|r| r.task_ir["task_type"].as_str() == Some(t.id));
            let Some(src) = src else {
                match fixture {
                    Some(f) if f.task_type == t.id => { via = "scripted_fixture"; scripted(f, arm, &mut attempts, &mut passes, &mut stopped) }
                    _ => {
                        arm.skipped.push(json!({ "case": key, "reason": "source run not found, unfinished or of another task type" }));
                        return Ok(());
                    }
                }
                return finish_case(key, case, via, attempts, passes, stopped, arm);
            };
            via = "replay";
            let ov = Overrides { graph: Some(GraphOverride::Inline(graph.clone())), ..Default::default() };
            let source = evals::source_trajectory(st, &src).await;
            for i in 0..case.k {
                let rpl = new_id("rpl");
                match replays::replay_one(st, &src, &ov, &rpl, None).await {
                    Ok(o) => {
                        for d in o.results["divergences"].as_array().into_iter().flatten() {
                            let node = d["step_ref"].as_str().unwrap_or_default().split('#').next().unwrap_or_default().to_string();
                            *arm.divergences.entry(node).or_default() += 1;
                        }
                        let tr = evals::replay_trajectory(&o.results, &source);
                        let (pass, grades) = graders::grade_all(&gs, &tr);
                        passes += pass as u32;
                        if !pass {
                            stopped = tr.stopped_at.clone();
                        }
                        add_trajectory(arm, graph, &tr);
                        let mut a = evals::attempt_json(i, pass, &grades);
                        a["verdict"] = json!(o.verdict);
                        attempts.push(a);
                    }
                    Err(f) if f.code == "ERR_BUDGET_CAP_REACHED" => return Err(f),
                    // A graph that cannot run this run's goal fails the attempt.
                    Err(f) => attempts.push(json!({ "attempt": i + 1, "pass": false, "error": f.json() })),
                }
            }
        }
    }
    finish_case(key, case, via, attempts, passes, stopped, arm)
}

fn finish_case(key: &str, case: &Case, via: &str, attempts: Vec<Value>, passes: u32, stopped: Option<String>, arm: &mut Arm) -> Result<(), Fail> {
    let pass = case.rule.passes(passes, case.k);
    arm.outcomes.insert(key.to_string(), pass);
    if case.high_stakes {
        arm.high_stakes.insert(key.to_string());
    }
    if !pass {
        *arm.failed_at.entry(stopped.unwrap_or_else(|| "unknown".into())).or_default() += 1;
    }
    arm.cases.push(json!({ "case": key, "via": via, "pass": pass, "passes": passes, "k": case.k, "attempts": attempts }));
    Ok(())
}

/// Evaluate `graph` on every case.
pub async fn evaluate(env: &Env<'_>, t: &TaskType, graph: &Value, cases: &[(String, Case)]) -> Result<Arm, Fail> {
    let mut arm = Arm::default();
    for (key, case) in cases {
        eval_case(env, t, graph, key, case, &mut arm).await?;
    }
    Ok(arm)
}

/// The cases a search evaluates on: the shipped eval set of the type, the
/// requested suites (cases of this type), and replays of recent real runs.
pub fn cases_for(t: &TaskType, suites: &[Suite], runs: &[String]) -> Vec<(String, Case)> {
    let mut out = vec![];
    let shipped = evals::SHIPPED_EVALS.iter().map(|r| evals::shipped_suite(r)).find(|s| s.id == format!("task_types.{}", t.id));
    for s in shipped.iter().chain(suites) {
        for c in &s.cases {
            let of_type = match &c.source {
                Source::Scripted(f) => f.task_type == t.id,
                Source::Run { fixture, .. } => fixture.as_ref().is_none_or(|f| f.task_type == t.id),
            };
            if of_type {
                out.push((format!("{}/{}", s.id, c.id), c.clone()));
            }
        }
    }
    for r in runs {
        out.push((format!("runs/{r}"), Case {
            id: r.clone(), source: Source::Run { run_id: r.clone(), fixture: None }, graders: vec![json!({ "type": "verifier_pass" })],
            k: 1, rule: PassRule::AllOfK, high_stakes: false, overrides: Value::Null, raw: Value::Null,
        }));
    }
    out
}

// ── versions ────────────────────────────────────────────────────────────────

/// The shipped graph JSON of `t` (version 1).
pub fn shipped_graph(t: &TaskType) -> Value {
    serde_json::to_value(task_types::graph(t).expect("shipped graphs parse")).unwrap_or(Value::Null)
}

pub async fn registry(s: &AgencyStore, t: &TaskType) -> anyhow::Result<Value> {
    Ok(s.load_object(EV_HARNESS_VERSIONS_STATE, t.id, SYSTEM_OWNER).await?.unwrap_or_else(|| json!({
        "object": "harness_versions", "task_type": t.id, "shipped": { "version": 1, "graph_id": t.graph_id },
        "active_version": 1, "stack": [1], "versions": [], "updated_at": null,
    })))
}

/// `(version, graph_id, unbound graph JSON)` new runs of `t` use.
pub async fn active(s: &AgencyStore, t: &TaskType) -> anyhow::Result<(u64, String, Value)> {
    let reg = registry(s, t).await?;
    let v = reg["active_version"].as_u64().unwrap_or(1);
    match reg["versions"].as_array().into_iter().flatten().find(|x| x["version"].as_u64() == Some(v)) {
        Some(x) if v > 1 => Ok((v, x["graph_id"].as_str().unwrap_or_default().to_string(), x["graph"].clone())),
        _ => Ok((1, t.graph_id.to_string(), shipped_graph(t))),
    }
}

/// At run creation: when an approved harness version is active for the
/// run's task type, re-instantiate the TaskIR's graph from it and pin it
/// (`task_ir.graph_version`), so the executor, the trace monitor and replays
/// use the version the run started with. Shipped version: no change.
pub async fn pin_active_version(s: &AgencyStore, task_ir: &mut Value) -> anyhow::Result<()> {
    let Some(t) = task_ir["task_type"].as_str().filter(|t| *t != "BUG_FIX").and_then(task_types::get) else { return Ok(()) };
    let (version, graph_id, graph) = active(s, t).await?;
    if version <= 1 {
        return Ok(());
    }
    let task_id = task_ir["wih_policy"]["task_id"].as_str().unwrap_or_default().to_string();
    let writable: Vec<String> = task_ir["wih_policy"]["write_set"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
    let pinned = json!({ "task_type": t.id, "graph_id": graph_id, "version": version, "graph": graph });
    let mut probe = json!({});
    probe[task_types::GRAPH_VERSION_KEY] = pinned.clone();
    let g = task_types::instantiate_for_ir(t, &probe, &task_id, &writable)?;
    let tg = task_types::template_graph_of(&g, &writable, &graph_id, &task_id)?;
    task_ir["nodes"] = json!(tg.nodes);
    task_ir["edges"] = json!(tg.edges);
    task_ir["wih_policy"]["graph_id"] = json!(graph_id);
    task_ir[task_types::GRAPH_VERSION_KEY] = pinned;
    Ok(())
}

async fn audit(s: &AgencyStore, action: &str, actor: &str, detail: Value) -> anyhow::Result<()> {
    let id = new_id("hau");
    let mut o = json!({ "id": id, "object": "harness_audit", "action": action, "actor": actor, "at": now() });
    if let (Some(a), Some(d)) = (o.as_object_mut(), detail.as_object()) {
        a.extend(d.clone());
    }
    s.save_object(EV_HARNESS_AUDIT_STATE, &id, SYSTEM_OWNER, &o).await
}

// ── evidence ────────────────────────────────────────────────────────────────

fn s1_kinds(graph: &Value) -> BTreeMap<String, (String, f64)> {
    graph["nodes"].as_array().into_iter().flatten()
        .filter(|n| n["cognitive_role"] == "S1")
        .filter_map(|n| {
            let spec = n["extensions"].get(s1::SPEC_KEY)?;
            Some((n["node_id"].as_str()?.to_string(), (spec["kind"].as_str().unwrap_or("choice").to_string(),
                spec["min_confidence"].as_f64().unwrap_or(mutate::DEFAULT_MIN_CONFIDENCE))))
        })
        .collect()
}

/// Decision Runtime metrics (all owners, last 30 days) per S1 kind of the graph.
fn descent(st: &AppState, graph: &Value) -> Value {
    let kinds = s1_kinds(graph);
    let since = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let Ok(conn) = st.db.connect() else { return json!({}) };
    let mut out = Map::new();
    let uniq: BTreeSet<(String, u64)> = kinds.values().map(|(k, m)| (k.clone(), (m * 1000.0) as u64)).collect();
    for (kind, mc) in uniq {
        let mc = mc as f64 / 1000.0;
        let r = conn.query_row(
            "SELECT COUNT(*), COALESCE(AVG(confidence), 0), COALESCE(SUM(abstained), 0), COALESCE(SUM(confidence < ?3), 0),
                    COALESCE(SUM(backend = 'oracle'), 0) FROM decisions WHERE kind = ?1 AND created_at >= ?2",
            rusqlite::params![kind, since, mc],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?)),
        );
        if let Ok((n, conf, abst, low, oracle)) = r {
            let share = |x: i64| if n > 0 { stats::round4(x as f64 / n as f64) } else { 0.0 };
            out.insert(kind, json!({ "decisions": n, "mean_confidence": stats::round4(conf), "abstain_share": share(abst),
                "below_min_confidence_share": share(low), "oracle_share": share(oracle), "min_confidence": mc }));
        }
    }
    Value::Object(out)
}

/// Recent runs of the type, their trace-monitor findings, the admin's
/// recent eval runs and descent metrics.
async fn gather_evidence(st: &Arc<AppState>, user: &AuthUser, t: &TaskType, graph: &Value) -> anyhow::Result<(Value, Vec<String>)> {
    let s = store(st);
    let runs = replays::select_recent(s.runs_with_status(replays::FINISHED).await?, Some(t.id), None, EVIDENCE_RUNS);
    let run_ids: Vec<String> = runs.iter().filter_map(|r| r.run["id"].as_str().map(String::from)).collect();
    let mut by_check: BTreeMap<String, Value> = BTreeMap::new();
    let mut by_node: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut statuses: BTreeMap<String, u64> = BTreeMap::new();
    for r in &runs {
        *statuses.entry(r.run["status"].as_str().unwrap_or("unknown").to_string()).or_default() += 1;
        let id = r.run["id"].as_str().unwrap_or_default();
        let Some(f) = monitor::stored(&s, id, &r.owner).await? else { continue };
        for x in f["data"].as_array().into_iter().flatten() {
            let check = x["check"].as_str().unwrap_or_default().to_string();
            let e = by_check.entry(check.clone()).or_insert_with(|| json!({ "count": 0, "severity": x["severity"] }));
            e["count"] = json!(e["count"].as_u64().unwrap_or(0) + 1);
            for n in x["evidence"]["nodes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                *by_node.entry(n.to_string()).or_default().entry(check.clone()).or_default() += 1;
            }
        }
    }
    let mut evals_ev = vec![];
    for er in s.list_objects(EV_EVAL_RUN_STATE, &user.user_id).await?.into_iter().take(EVIDENCE_EVAL_RUNS) {
        let cases: Vec<&Value> = er["suites"].as_array().into_iter().flatten().flat_map(|su| su["cases"].as_array().into_iter().flatten())
            .filter(|c| c["task_type"] == t.id && c["skipped"] != true).collect();
        if cases.is_empty() {
            continue;
        }
        let failed: Vec<&Value> = cases.iter().filter(|c| c["pass"] != true).map(|c| &c["case_id"]).collect();
        evals_ev.push(json!({ "eval_run_id": er["id"], "cases": cases.len(), "failed": failed }));
    }
    let ev = json!({
        "runs": { "considered": runs.len(), "by_status": statuses },
        "findings": { "by_check": by_check, "by_node": by_node },
        "eval_runs": evals_ev,
        "descent": descent(st, graph),
    });
    Ok((ev, run_ids))
}

/// Re-rank deterministic candidates by the evidence (higher first).
fn score(c: &mut Candidate, ev: &Value, base: &Arm) {
    let target = c.target.clone().unwrap_or_default();
    let node_checks = &ev["findings"]["by_node"][&target];
    let kind = |g: &Value| s1_kinds(g).get(&target).map(|(k, _)| k.clone()).unwrap_or_default();
    match c.operator.as_str() {
        "min_confidence_up" => {
            let d = &ev["descent"][kind(&c.graph)];
            if node_checks.get("s1_low_confidence").is_some() || d["below_min_confidence_share"].as_f64().unwrap_or(0.0) > 0.2 {
                c.score += 2.0;
            }
        }
        "min_confidence_down" => {
            let d = &ev["descent"][kind(&c.graph)];
            if node_checks.get("escalation_storm").is_some() || d["abstain_share"].as_f64().unwrap_or(0.0) > 0.3 {
                c.score += 2.0;
            }
        }
        "add_verifier" => {
            if base.failed_at.keys().chain(base.divergences.keys()).any(|n| n.starts_with(&target)) || node_checks.is_object() {
                c.score += 1.5;
            }
        }
        _ => {}
    }
}

// ── LLM proposer ────────────────────────────────────────────────────────────

/// A text-completion model for the LLM proposer (a trait so tests stub it).
#[async_trait]
pub trait Proposer: Send + Sync {
    /// `(reply, tokens, cost_usd)`.
    async fn complete(&self, prompt: &str) -> Result<(String, u64, f64), String>;
}

pub const PROPOSER_SYSTEM: &str = "You improve agent workflow graphs (ComputeGraphIRV1 JSON). Reply with JSON only: \
{\"candidates\": [{\"rationale\": \"...\", \"patch\": [RFC 6902 operations against the given graph]}]}. \
Never change the completion gate, POLICY or WAIT nodes, completion_nodes, write_set, lock_scope or x-bind_write_set. \
Only use primitives that already appear in the graph. Prefer converting model (S2) checks into S1 decisions with an S2 twin, \
tuning S1 min_confidence, adding deterministic S0 verify steps, and removing steps nothing reads.";

/// The oracle/planner model through gizzi-code, tools off, metered on the
/// usage ledger (`surface = replay`), behind the daily spend caps.
pub struct GizziProposer {
    pub caps: replays::sim::SpendGate,
    pub ledger: crate::usage_ledger::LedgerCtx,
}

#[async_trait]
impl Proposer for GizziProposer {
    async fn complete(&self, prompt: &str) -> Result<(String, u64, f64), String> {
        self.caps.admit().await?;
        let (text, usage) = crate::usage_ledger::scope(self.ledger.clone(), crate::gizzi_completion::complete_ephemeral_usage(prompt, Some(PROPOSER_SYSTEM), None)).await?;
        Ok((text, usage.tokens, usage.cost_usd))
    }
}

/// Scripted cognition: no model call, no candidates.
pub struct NoProposer;

#[async_trait]
impl Proposer for NoProposer {
    async fn complete(&self, _prompt: &str) -> Result<(String, u64, f64), String> {
        Ok((r#"{"candidates": []}"#.into(), 0, 0.0))
    }
}

fn truncate(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// One proposer call: parse its patches, apply each to `base`, keep what
/// validates. Returns `(candidates, discarded, report)`.
pub async fn llm_candidates(p: &dyn Proposer, t: &TaskType, base: &Value, evidence: &Value, k: usize) -> (Vec<Candidate>, Vec<Value>, Value) {
    let prompt = format!(
        "Task type: {}\nPropose up to {k} edits that raise the eval pass rate or cut cost/latency without losing quality.\n\nGraph:\n{}\n\nEvidence:\n{}\n",
        t.id, base, truncate(evidence.to_string(), EVIDENCE_PROMPT_BYTES));
    let (text, tokens, cost) = match p.complete(&prompt).await {
        Ok(x) => x,
        Err(e) => return (vec![], vec![], json!({ "used": true, "error": e, "candidates": 0 })),
    };
    let body = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => serde_json::from_str::<Value>(&text[a..=b]).ok(),
        _ => None,
    };
    let Some(items) = body.as_ref().and_then(|b| b["candidates"].as_array()) else {
        return (vec![], vec![json!({ "operator": "llm_patch", "reason": "the proposer reply is not {\"candidates\": [...]} JSON" })],
                json!({ "used": true, "tokens": tokens, "cost_usd": cost, "candidates": 0 }));
    };
    let (mut ok, mut bad) = (vec![], vec![]);
    for (i, it) in items.iter().take(k).enumerate() {
        let rationale = it["rationale"].as_str().unwrap_or("LLM proposal").chars().take(500).collect::<String>();
        let r = mutate::apply_patch(base, &it["patch"]).and_then(|g| mutate::validate(t, base, &g).map(|_| g));
        match r {
            Ok(g) => ok.push(Candidate { operator: "llm_patch".into(), target: Some(format!("llm#{i}")), rationale, graph: g, patch: Some(it["patch"].clone()), score: 2.5 }),
            Err(e) => bad.push(json!({ "operator": "llm_patch", "target": format!("llm#{i}"), "reason": e, "patch": it["patch"] })),
        }
    }
    let report = json!({ "used": true, "tokens": tokens, "cost_usd": cost, "returned": items.len(), "candidates": ok.len() });
    (ok, bad, report)
}

// ── the search ──────────────────────────────────────────────────────────────

/// A parsed `POST /v1/harness/proposals` request.
pub struct Request {
    pub task_type: &'static TaskType,
    pub k: usize,
    pub operators: Vec<String>,
    pub llm: bool,
    pub suites: Vec<Suite>,
    pub replay_runs: usize,
    pub rule: Rule,
    pub seed: u64,
    pub resamples: usize,
}

fn parse_request(obj: &Map<String, Value>) -> Result<(Request, Vec<String>), Fail> {
    const KEYS: &[&str] = &["task_type", "k", "operators", "llm", "suite_ids", "replay_runs", "acceptance", "seed", "resamples", "effects"];
    if let Some(k) = obj.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(Fail::unsupported(k, format!("unknown field `{k}`")));
    }
    replays::check_effects(obj)?;
    let tt = obj.get("task_type").and_then(Value::as_str).map(str::to_ascii_uppercase).unwrap_or_default();
    let t = task_types::get(&tt).ok_or_else(|| Fail::input("task_type", format!("task_type must be one of {}", task_types::ids().join(", "))))?;
    let int = |k: &str, lo: usize, hi: usize, d: usize| -> Result<usize, Fail> {
        match obj.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => v.as_u64().map(|x| x as usize).filter(|x| (lo..=hi).contains(x)).ok_or_else(|| Fail::input(k, format!("{k} must be {lo}-{hi}"))),
        }
    };
    let operators = match obj.get("operators") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => {
            let ops: Vec<String> = a.iter().filter_map(Value::as_str).map(String::from).collect();
            if ops.len() != a.len() || ops.iter().any(|o| !mutate::OPERATORS.contains(&o.as_str())) {
                return Err(Fail::input("operators", format!("operators are some of {}", mutate::OPERATORS.join(", "))));
            }
            ops
        }
        Some(_) => return Err(Fail::input("operators", "operators must be an array")),
    };
    let suite_ids: Vec<String> = match obj.get("suite_ids") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) if a.len() <= MAX_SUITES && a.iter().all(Value::is_string) => a.iter().filter_map(Value::as_str).map(String::from).collect(),
        Some(_) => return Err(Fail::input("suite_ids", format!("suite_ids lists at most {MAX_SUITES} suite ids"))),
    };
    let req = Request {
        task_type: t,
        k: int("k", 1, MAX_K, DEFAULT_K)?,
        operators,
        llm: match obj.get("llm") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(Fail::input("llm", "llm must be a boolean")),
        },
        suites: vec![],
        replay_runs: int("replay_runs", 0, MAX_REPLAY_RUNS, DEFAULT_REPLAY_RUNS)?,
        rule: Rule::parse(obj.get("acceptance"))?,
        seed: match obj.get("seed") {
            None | Some(Value::Null) => stats::DEFAULT_SEED,
            Some(v) => v.as_u64().ok_or_else(|| Fail::input("seed", "seed must be a non-negative integer"))?,
        },
        resamples: int("resamples", 100, stats::MAX_RESAMPLES, stats::DEFAULT_RESAMPLES)?,
    };
    Ok((req, suite_ids))
}

/// Generate, validate, evaluate and judge candidates; store the proposals.
/// `search` is the stored search object (status `running`); returns it
/// settled.
pub async fn run_search(env: &Env<'_>, s: &AgencyStore, user: &str, req: &Request, proposer: &dyn Proposer, evidence: Value, run_ids: &[String], mut search: Value) -> Value {
    let t = req.task_type;
    let search_id = search["id"].as_str().unwrap_or_default().to_string();
    let (base_version, base_graph_id, base) = match active(s, t).await {
        Ok(x) => x,
        Err(e) => return failed(search, Fail::new(500, "ERR_INTERNAL", e.to_string())),
    };
    search["base"] = json!({ "version": base_version, "graph_id": base_graph_id });
    let cases = cases_for(t, &req.suites, run_ids);
    let baseline = match evaluate(env, t, &base, &cases).await {
        Ok(a) => a,
        Err(f) => return failed(search, f),
    };
    let mut evidence = evidence;
    evidence["baseline_eval"] = json!({ "failed_at": baseline.failed_at, "divergences_by_node": baseline.divergences,
        "pass_rate": baseline.pass_rate().map(stats::round4) });

    // Candidates: deterministic operators, then (optionally) the LLM.
    let mut discarded = vec![];
    let mut cands: Vec<Candidate> = vec![];
    for mut c in mutate::mutations(&base, &req.operators) {
        match mutate::validate(t, &base, &c.graph) {
            Ok(()) => {
                score(&mut c, &evidence, &baseline);
                cands.push(c);
            }
            Err(e) => discarded.push(json!({ "operator": c.operator, "target": c.target, "reason": e })),
        }
    }
    let mut llm_report = json!({ "used": false });
    if req.llm {
        let (ok, bad, rep) = llm_candidates(proposer, t, &base, &evidence, req.k).await;
        cands.extend(ok);
        discarded.extend(bad);
        llm_report = rep;
    }
    // Dedup (and drop no-ops), rank, keep k.
    let mut seen: Vec<Value> = vec![base.clone()];
    cands.retain(|c| {
        if seen.contains(&c.graph) {
            false
        } else {
            seen.push(c.graph.clone());
            true
        }
    });
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));
    let over: Vec<Candidate> = cands.split_off(cands.len().min(req.k));
    let not_evaluated: Vec<Value> = over.iter().map(|c| json!({ "operator": c.operator, "target": c.target, "score": c.score })).collect();

    let mut results = vec![];
    let mut pending = vec![];
    for (i, mut c) in cands.into_iter().enumerate() {
        c.graph["graph_id"] = json!(format!("{}.candidate", mutate::graph_stem(&base_graph_id)));
        let arm = match evaluate(env, t, &c.graph, &cases).await {
            Ok(a) => a,
            Err(f) => return failed(search, f),
        };
        let verdict = decide(&req.rule, &baseline, &arm, req.seed.wrapping_add(i as u64), req.resamples);
        let passed = verdict["passed"] == true;
        let pid = new_id("hpr");
        let status = if passed { "pending" } else { "failed_acceptance" };
        let p = json!({
            "id": pid, "object": "harness_proposal", "search_id": search_id, "task_type": t.id, "status": status,
            "base": { "version": base_version, "graph_id": base_graph_id },
            "operator": c.operator, "target": c.target, "rationale": c.rationale, "patch": c.patch,
            "acceptance": verdict, "metrics": { "baseline": baseline.summary(), "candidate": arm.summary() },
            "cases": { "baseline": baseline.cases, "candidate": arm.cases },
            "diff": mutate::diff(&base, &c.graph), "graph": c.graph, "evidence": evidence,
            "created_by": user, "created_at": now(), "decided_by": null, "decided_at": null, "version": null,
            "history": [{ "status": status, "at": now(), "by": user }],
        });
        if let Err(e) = s.save_object(EV_HARNESS_PROPOSAL_STATE, &pid, SYSTEM_OWNER, &p).await {
            return failed(search, Fail::new(500, "ERR_INTERNAL", e.to_string()));
        }
        let _ = audit(s, "proposal.created", user, json!({ "proposal_id": pid, "task_type": t.id, "status": status, "operator": c.operator })).await;
        if passed {
            pending.push(json!(pid));
        }
        results.push(json!({ "proposal_id": pid, "operator": c.operator, "target": c.target, "status": status,
            "passed": passed, "reasons": p["acceptance"]["reasons"], "delta": p["acceptance"]["delta"] }));
    }
    search["status"] = json!("completed");
    search["finished_at"] = json!(now());
    search["evidence"] = evidence;
    search["baseline"] = baseline.summary();
    search["cases"] = json!(cases.iter().map(|(k, _)| k).collect::<Vec<_>>());
    search["candidates"] = json!(results);
    search["discarded"] = json!(discarded);
    search["not_evaluated"] = json!(not_evaluated);
    search["llm"] = llm_report;
    search["pending"] = json!(pending);
    search
}

fn failed(mut search: Value, f: Fail) -> Value {
    search["status"] = json!("failed");
    search["finished_at"] = json!(now());
    search["error"] = f.json();
    search
}

// ── handlers ────────────────────────────────────────────────────────────────

fn require_admin(user: &AuthUser, rid: &RequestId) -> Result<(), ApiError> {
    if flywheel::is_flywheel_admin(&user.user_id) {
        Ok(())
    } else {
        Err(ApiError::new(403, "PERMISSION", "ERR_PERMISSION_DENIED", "only a decisions admin can propose, approve or roll back harness changes", rid))
    }
}

fn body_obj(body: &axum::body::Bytes, rid: &RequestId) -> Result<Map<String, Value>, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Map::new());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(o)) => Ok(o),
        Ok(_) => Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "body must be an object", rid)),
        Err(e) => Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", format!("invalid JSON: {e}"), rid)),
    }
}

/// `POST /v1/harness/proposals` (decisions admins).
pub async fn create_proposals(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    body: axum::body::Bytes,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let obj = body_obj(&body, &rid)?;
    let (mut req, suite_ids) = parse_request(&obj).map_err(|f| f.api(&rid))?;
    let s = store(&st);
    for id in &suite_ids {
        let v = s.load_object(EV_EVAL_SUITE_STATE, id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
            .ok_or_else(|| ApiError::not_found(&format!("eval suite {id}"), &rid))?;
        let mut raw = v.clone();
        if let Some(o) = raw.as_object_mut() {
            for k in ["object", "owner", "case_count", "updated_at", "created_at"] {
                o.remove(k);
            }
        }
        req.suites.push(evals::parse_suite(&raw).map_err(|f| f.api(&rid))?);
    }
    let (_, _, base) = active(&s, req.task_type).await.map_err(|e| ApiError::internal(e, &rid))?;
    let (evidence, mut run_ids) = gather_evidence(&st, &user, req.task_type, &base).await.map_err(|e| ApiError::internal(e, &rid))?;
    run_ids.truncate(req.replay_runs);
    let id = new_id("hsr");
    let search = json!({ "id": id, "object": "harness_search", "status": "running", "task_type": req.task_type.id, "k": req.k,
        "operators": if req.operators.is_empty() { json!(mutate::OPERATORS) } else { json!(req.operators) }, "llm_requested": req.llm,
        "suite_ids": suite_ids, "replay_runs": run_ids, "acceptance_rule": req.rule.json(), "seed": req.seed, "effects": "recorded_only",
        "created_by": user.user_id, "created_at": now(), "finished_at": null, "error": null });
    s.save_object(EV_HARNESS_SEARCH_STATE, &id, SYSTEM_OWNER, &search).await.map_err(|e| ApiError::internal(e, &rid))?;
    let org = guard::org_of(&user);
    let live_llm = req.llm && !executor::scripted();
    let proposer: Box<dyn Proposer> = if live_llm {
        let caps = replays::sim::SpendGate::new(s.clone(), st.db.clone(), &org, guard::Limits::from_env());
        caps.admit().await.map_err(|why| Fail::new(429, "ERR_BUDGET_CAP_REACHED", why).api(&rid))?;
        let ledger = crate::usage_ledger::LedgerCtx::surface("replay").tenant(Some(&org), Some(&user.user_id));
        Box::new(GizziProposer { caps, ledger })
    } else {
        Box::new(NoProposer)
    };
    // Live model calls (LLM proposer, or replays that regenerate cognition) run in the background.
    let live = live_llm || (!run_ids.is_empty() && !executor::scripted());
    if live {
        let st2 = st.clone();
        let shown = search.clone();
        tokio::spawn(async move {
            let s = store(&st2);
            let done = run_search(&Env::Online { st: &st2 }, &s, &user.user_id, &req, proposer.as_ref(), evidence, &run_ids, search).await;
            if let Err(e) = s.save_object(EV_HARNESS_SEARCH_STATE, &id, SYSTEM_OWNER, &done).await {
                tracing::warn!(search = %id, error = %e, "harness search not stored");
            }
        });
        return Ok((StatusCode::ACCEPTED, Json(shown)).into_response());
    }
    let done = run_search(&Env::Online { st: &st }, &s, &user.user_id, &req, proposer.as_ref(), evidence, &run_ids, search).await;
    s.save_object(EV_HARNESS_SEARCH_STATE, &id, SYSTEM_OWNER, &done).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::CREATED, Json(done)).into_response())
}

pub async fn get_search(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let v = store(&st).load_object(EV_HARNESS_SEARCH_STATE, &id, SYSTEM_OWNER).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("harness search", &rid))?;
    Ok(Json(v).into_response())
}

fn proposal_brief(p: &Value) -> Value {
    json!({ "id": p["id"], "object": "harness_proposal", "task_type": p["task_type"], "status": p["status"], "operator": p["operator"],
            "target": p["target"], "rationale": p["rationale"], "base": p["base"], "version": p["version"],
            "acceptance": { "passed": p["acceptance"]["passed"], "reasons": p["acceptance"]["reasons"], "delta": p["acceptance"]["delta"] },
            "diff_summary": p["diff"]["summary"], "created_at": p["created_at"], "decided_at": p["decided_at"] })
}

/// `GET /v1/harness/proposals?status=pending&task_type=...` (decisions admins).
pub async fn list_proposals(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let v = store(&st).list_objects(EV_HARNESS_PROPOSAL_STATE, SYSTEM_OWNER).await.map_err(|e| ApiError::internal(e, &rid))?;
    let tt = q.get("task_type").map(|t| t.to_ascii_uppercase());
    let v = v.iter()
        .filter(|p| q.get("status").is_none_or(|s| p["status"] == s.as_str()))
        .filter(|p| tt.as_ref().is_none_or(|t| p["task_type"] == t.as_str()))
        .map(proposal_brief).collect();
    Ok(Json(page(v)).into_response())
}

pub async fn get_proposal(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let v = store(&st).load_object(EV_HARNESS_PROPOSAL_STATE, &id, SYSTEM_OWNER).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("harness proposal", &rid))?;
    Ok(Json(v).into_response())
}

fn note_of(obj: &Map<String, Value>, rid: &RequestId) -> Result<Option<String>, ApiError> {
    if let Some(k) = obj.keys().find(|k| k.as_str() != "note") {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", format!("unknown field `{k}`"), rid).param(Some(k.clone())));
    }
    match obj.get("note") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.len() <= 2000 => Ok(Some(s.clone())),
        Some(_) => Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "note must be a string of at most 2000 bytes", rid).param(Some("note".into()))),
    }
}

async fn load_pending(s: &AgencyStore, id: &str, rid: &RequestId) -> Result<Value, ApiError> {
    let p = s.load_object(EV_HARNESS_PROPOSAL_STATE, id, SYSTEM_OWNER).await.map_err(|e| ApiError::internal(e, rid))?
        .ok_or_else(|| ApiError::not_found("harness proposal", rid))?;
    if p["status"] != "pending" {
        return Err(ApiError::new(409, "STATE", "ERR_HARNESS_NOT_PENDING", format!("proposal is {}; only pending proposals can be decided",
            p["status"].as_str().unwrap_or("unknown")), rid));
    }
    Ok(p)
}

fn decided(mut p: Value, status: &str, by: &str, note: Option<String>) -> Value {
    let at = now();
    p["status"] = json!(status);
    p["decided_by"] = json!(by);
    p["decided_at"] = json!(at);
    if let Some(h) = p["history"].as_array_mut() {
        h.push(json!({ "status": status, "at": at, "by": by, "note": note }));
    }
    p
}

/// `POST /v1/harness/proposals/:id/approve`: register the candidate as the
/// task type's next graph version and make it active for new runs.
pub async fn approve_proposal(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let note = note_of(&body_obj(&body, &rid)?, &rid)?;
    let s = store(&st);
    let _g = s.lock().await;
    let p = load_pending(&s, &id, &rid).await?;
    let t = task_types::require(p["task_type"].as_str().unwrap_or_default()).map_err(|e| ApiError::internal(e, &rid))?;
    let mut reg = registry(&s, t).await.map_err(|e| ApiError::internal(e, &rid))?;
    let active_v = reg["active_version"].as_u64().unwrap_or(1);
    if p["base"]["version"].as_u64() != Some(active_v) {
        return Err(ApiError::new(409, "STATE", "ERR_HARNESS_STALE", format!(
            "proposal was evaluated against version {} but version {active_v} is active now; propose again", p["base"]["version"]), &rid));
    }
    let (_, _, base) = active(&s, t).await.map_err(|e| ApiError::internal(e, &rid))?;
    let next = reg["versions"].as_array().into_iter().flatten().filter_map(|v| v["version"].as_u64()).max().unwrap_or(1).max(1) + 1;
    let graph_id = format!("{}.v{next}", mutate::graph_stem(t.graph_id));
    let mut graph = p["graph"].clone();
    graph["graph_id"] = json!(graph_id);
    // Fail closed: re-validate at approval.
    mutate::validate(t, &base, &graph).map_err(|e| ApiError::new(422, "STATE", "ERR_HARNESS_INVALID", e, &rid))?;
    let at = now();
    if let Some(a) = reg["versions"].as_array_mut() {
        a.push(json!({ "version": next, "graph_id": graph_id, "graph": graph, "proposal_id": id,
            "parent_version": active_v, "approved_by": user.user_id, "approved_at": at, "rolled_back_at": null }));
    }
    if let Some(a) = reg["stack"].as_array_mut() {
        a.push(json!(next));
    }
    reg["active_version"] = json!(next);
    reg["updated_at"] = json!(at);
    s.save_object(EV_HARNESS_VERSIONS_STATE, t.id, SYSTEM_OWNER, &reg).await.map_err(|e| ApiError::internal(e, &rid))?;
    let mut p = decided(p, "approved", &user.user_id, note.clone());
    p["version"] = json!({ "version": next, "graph_id": graph_id });
    s.save_object(EV_HARNESS_PROPOSAL_STATE, &id, SYSTEM_OWNER, &p).await.map_err(|e| ApiError::internal(e, &rid))?;
    audit(&s, "proposal.approved", &user.user_id, json!({ "proposal_id": id, "task_type": t.id, "version": next, "from_version": active_v,
        "graph_id": graph_id, "note": note })).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(json!({ "proposal": proposal_brief(&p), "versions": versions_view(&reg) })).into_response())
}

/// `POST /v1/harness/proposals/:id/reject`.
pub async fn reject_proposal(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let note = note_of(&body_obj(&body, &rid)?, &rid)?;
    let s = store(&st);
    let _g = s.lock().await;
    let p = load_pending(&s, &id, &rid).await?;
    let p = decided(p, "rejected", &user.user_id, note.clone());
    s.save_object(EV_HARNESS_PROPOSAL_STATE, &id, SYSTEM_OWNER, &p).await.map_err(|e| ApiError::internal(e, &rid))?;
    audit(&s, "proposal.rejected", &user.user_id, json!({ "proposal_id": id, "task_type": p["task_type"], "note": note })).await
        .map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(proposal_brief(&p)).into_response())
}

/// The registry without graph bodies.
fn versions_view(reg: &Value) -> Value {
    let mut r = reg.clone();
    for v in r["versions"].as_array_mut().into_iter().flatten() {
        if let Some(o) = v.as_object_mut() {
            o.remove("graph");
        }
    }
    r
}

fn task_type_of(tt: &str, rid: &RequestId) -> Result<&'static TaskType, ApiError> {
    task_types::get(&tt.to_ascii_uppercase()).ok_or_else(|| ApiError::not_found(&format!("task type {tt}"), rid))
}

/// `GET /v1/harness/versions/:task_type` (decisions admins).
pub async fn get_versions(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(tt): Path<String>,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let t = task_type_of(&tt, &rid)?;
    let reg = registry(&store(&st), t).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(versions_view(&reg)).into_response())
}

/// `POST /v1/harness/versions/:task_type/rollback`: new runs go back to the
/// version that was active before the current one.
pub async fn rollback(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(tt): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let note = note_of(&body_obj(&body, &rid)?, &rid)?;
    let t = task_type_of(&tt, &rid)?;
    let s = store(&st);
    let _g = s.lock().await;
    let mut reg = registry(&s, t).await.map_err(|e| ApiError::internal(e, &rid))?;
    let from = reg["active_version"].as_u64().unwrap_or(1);
    let mut stack: Vec<u64> = reg["stack"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
    if from <= 1 || stack.len() < 2 {
        return Err(ApiError::new(409, "STATE", "ERR_HARNESS_NOTHING_TO_ROLL_BACK", format!("{} runs the shipped graph; nothing to roll back", t.id), &rid));
    }
    stack.pop();
    let to = *stack.last().unwrap_or(&1);
    let at = now();
    let mut proposal_id = Value::Null;
    for v in reg["versions"].as_array_mut().into_iter().flatten().filter(|v| v["version"].as_u64() == Some(from)) {
        v["rolled_back_at"] = json!(at);
        proposal_id = v["proposal_id"].clone();
    }
    reg["stack"] = json!(stack);
    reg["active_version"] = json!(to);
    reg["updated_at"] = json!(at);
    s.save_object(EV_HARNESS_VERSIONS_STATE, t.id, SYSTEM_OWNER, &reg).await.map_err(|e| ApiError::internal(e, &rid))?;
    if let Some(pid) = proposal_id.as_str() {
        if let Some(p) = s.load_object(EV_HARNESS_PROPOSAL_STATE, pid, SYSTEM_OWNER).await.map_err(|e| ApiError::internal(e, &rid))? {
            let p = decided(p, "rolled_back", &user.user_id, note.clone());
            s.save_object(EV_HARNESS_PROPOSAL_STATE, pid, SYSTEM_OWNER, &p).await.map_err(|e| ApiError::internal(e, &rid))?;
        }
    }
    audit(&s, "version.rolled_back", &user.user_id, json!({ "task_type": t.id, "from_version": from, "version": to, "proposal_id": proposal_id, "note": note }))
        .await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(versions_view(&reg)).into_response())
}

/// `GET /v1/harness/audit?task_type=...` (decisions admins), newest first.
pub async fn list_audit(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    require_admin(&user, &rid)?;
    let tt = q.get("task_type").map(|t| t.to_ascii_uppercase());
    let v = store(&st).list_objects(EV_HARNESS_AUDIT_STATE, SYSTEM_OWNER).await.map_err(|e| ApiError::internal(e, &rid))?
        .into_iter().filter(|a| tt.as_ref().is_none_or(|t| a["task_type"] == t.as_str())).collect();
    Ok(Json(page(v)).into_response())
}
