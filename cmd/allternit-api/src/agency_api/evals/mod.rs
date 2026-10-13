//! C5: eval harness built on replay.
//!
//! Evals are graders applied to trajectories. A **suite** (JSON,
//! `allternit.eval_suite.v1`) lists cases; a case is either
//! - a recorded run (`source_run_id`), replayed `k` times through the
//!   recorded-only replay engine (`replays::replay_one`, the engine behind
//!   `POST /v1/replays/batch`), optionally with replay `overrides` (a
//!   candidate head, model class or graph) — every effect stays recorded_only;
//! - or a task-type goal fixture (`task_type`, `goal`, `params`, `script`),
//!   walked `k` times by the scripted runner (`task_types::runner`: no model,
//!   no effect).
//!
//! Each attempt is graded by deterministic trajectory graders ([`graders`]);
//! a case passes by its rule: `all_of_k` (high stakes) or `{"n_of_k": n}`.
//! Results carry per-case and aggregate pass rates with a percentile
//! bootstrap 95% interval (seeded, so reproducible: [`stats`]).
//!
//! `POST /v1/evals/cases/from-run/:run_id` promotes a production run (one the
//! trace monitor flagged, typically) into a suite as a regression case: its
//! ids, path, effect count, verdict and a scripted fixture, never its goal
//! text, outputs or credentials.
//!
//! The offline report ([`offline_report`]) runs the shipped task-type eval
//! sets and the committed suites with the scripted runner only; the CI gate
//! (`scripts/agency-eval-gate.mjs`) compares it with `baseline.v1.json`.

pub mod graders;
pub mod stats;
#[cfg(test)]
mod tests;

use super::decisions::flywheel;
use super::executor::generic::TASK_WAIT_REASON;
use super::replays::{self, Fail};
use super::store::{new_id, now, RunRecord, EV_EVAL_RUN_STATE, EV_EVAL_SUITE_STATE, EV_REPLAY_STATE};
use super::task_types::{self, runner};
use super::{monitor, owned, store, ApiError, ApiResult, RequestId};
use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use graders::{Effect, Step, Trajectory};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub const SUITE_SCHEMA: &str = "allternit.eval_suite.v1";
pub const REPORT_SCHEMA: &str = "allternit.eval_report.v1";
pub const MAX_CASES: usize = 100;
pub const MAX_K: u32 = 10;
pub const MAX_GRADERS: usize = 20;
/// The suite `from-run` promotes into when none is named.
pub const DEFAULT_REGRESSION_SUITE: &str = "regressions";

/// The shipped per-task-type eval sets (also run by `task_types::tests`).
pub const SHIPPED_EVALS: &[&str] = &[
    include_str!("../task_types/evals/general_task.v1.json"),
    include_str!("../task_types/evals/thread_work.v1.json"),
    include_str!("../task_types/evals/computer_use.v1.json"),
    include_str!("../task_types/evals/research_doc.v1.json"),
    include_str!("../task_types/evals/template.v1.json"),
    include_str!("../task_types/evals/campaign.v1.json"),
];

/// Suites committed with the code (`evals/suites/*.v1.json`), gated in CI.
pub const COMMITTED_SUITES: &[&str] = &[include_str!("suites/core.v1.json")];

// ── suites ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum PassRule {
    AllOfK,
    NOfK(u32),
}

impl PassRule {
    pub fn passes(&self, passes: u32, k: u32) -> bool {
        match self {
            PassRule::AllOfK => passes == k,
            PassRule::NOfK(n) => passes >= *n,
        }
    }
    pub fn json(&self) -> Value {
        match self {
            PassRule::AllOfK => json!("all_of_k"),
            PassRule::NOfK(n) => json!({ "n_of_k": n }),
        }
    }
}

/// A scripted goal fixture (no model, no effect).
#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    pub task_type: String,
    pub goal: String,
    pub params: Value,
    pub script: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A recorded run, replayed; `fixture` stands in when the recording is
    /// not here (another server, CI).
    Run { run_id: String, fixture: Option<Fixture> },
    Scripted(Fixture),
}

#[derive(Debug, Clone)]
pub struct Case {
    pub id: String,
    pub source: Source,
    pub graders: Vec<Value>,
    pub k: u32,
    pub rule: PassRule,
    pub high_stakes: bool,
    pub overrides: Value,
    /// The raw case (echoed on the suite).
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct Suite {
    pub id: String,
    pub name: String,
    pub cases: Vec<Case>,
    pub raw: Value,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

fn parse_fixture(v: &Value, param: &str) -> Result<Fixture, Fail> {
    let tt = v["task_type"].as_str().map(str::to_ascii_uppercase).unwrap_or_default();
    if task_types::get(&tt).is_none() {
        return Err(Fail::input(&format!("{param}.task_type"), format!("unknown task_type (known: {})", task_types::ids().join(", "))));
    }
    let goal = match &v["goal"] {
        Value::Null => String::new(),
        Value::String(g) if g.len() <= 2000 => g.clone(),
        _ => return Err(Fail::input(&format!("{param}.goal"), "goal must be a string of at most 2000 bytes")),
    };
    let params = match &v["params"] {
        Value::Null => json!({}),
        p @ Value::Object(_) => p.clone(),
        _ => return Err(Fail::input(&format!("{param}.params"), "params must be an object")),
    };
    let script = match &v["script"] {
        Value::Null => json!({}),
        s => {
            serde_json::from_value::<runner::Script>(s.clone()).map_err(|e| Fail::input(&format!("{param}.script"), format!("invalid script: {e}")))?;
            s.clone()
        }
    };
    Ok(Fixture { task_type: tt, goal, params, script })
}

fn parse_case(v: &Value, i: usize) -> Result<Case, Fail> {
    let p = |k: &str| format!("cases[{i}].{k}");
    let o = v.as_object().ok_or_else(|| Fail::input(&format!("cases[{i}]"), "a case is an object"))?;
    const KEYS: &[&str] = &["id", "title", "source_run_id", "fixture", "task_type", "goal", "params", "script", "graders", "k", "pass_rule", "stakes", "overrides", "snapshot"];
    if let Some(x) = o.keys().find(|x| !KEYS.contains(&x.as_str())) {
        return Err(Fail::unsupported(&p(x), format!("unknown case field `{x}`")));
    }
    let id = v["id"].as_str().filter(|s| valid_id(s)).ok_or_else(|| Fail::input(&p("id"), "case id must be 1-64 characters of [A-Za-z0-9_.-]"))?.to_string();
    let source = match (v["source_run_id"].as_str(), v.get("task_type")) {
        (Some(_), Some(_)) => return Err(Fail::input(&p("source_run_id"), "give source_run_id or task_type, not both")),
        (Some(r), None) => {
            if !r.starts_with("run_") || r.len() > 128 {
                return Err(Fail::input(&p("source_run_id"), "source_run_id must be a run id"));
            }
            let fixture = match v.get("fixture") {
                None | Some(Value::Null) => None,
                Some(f) => Some(parse_fixture(f, &p("fixture"))?),
            };
            Source::Run { run_id: r.to_string(), fixture }
        }
        (None, Some(_)) => {
            if v.get("overrides").is_some_and(|o| !o.is_null()) {
                return Err(Fail::input(&p("overrides"), "overrides apply to replayed (source_run_id) cases"));
            }
            Source::Scripted(parse_fixture(v, &format!("cases[{i}]"))?)
        }
        (None, None) => return Err(Fail::input(&p("source_run_id"), "a case needs source_run_id (a recorded run) or task_type (a goal fixture)")),
    };
    let gs = v["graders"].as_array().filter(|a| !a.is_empty() && a.len() <= MAX_GRADERS)
        .ok_or_else(|| Fail::input(&p("graders"), format!("graders must list 1-{MAX_GRADERS} graders")))?;
    for (j, g) in gs.iter().enumerate() {
        graders::parse(g).map_err(|e| Fail::input(&format!("cases[{i}].graders[{j}]"), e))?;
    }
    let k = match &v["k"] {
        Value::Null => 1,
        x => x.as_u64().filter(|k| (1..=MAX_K as u64).contains(k)).ok_or_else(|| Fail::input(&p("k"), format!("k must be 1-{MAX_K}")))? as u32,
    };
    let rule = match &v["pass_rule"] {
        Value::Null => PassRule::AllOfK,
        Value::String(s) if s == "all_of_k" => PassRule::AllOfK,
        Value::Object(r) if r.len() == 1 && r.contains_key("n_of_k") => {
            let n = r["n_of_k"].as_u64().filter(|n| *n >= 1 && *n <= k as u64)
                .ok_or_else(|| Fail::input(&p("pass_rule"), format!("n_of_k must be 1-{k} (the case's k)")))?;
            PassRule::NOfK(n as u32)
        }
        _ => return Err(Fail::input(&p("pass_rule"), "pass_rule is \"all_of_k\" or {\"n_of_k\": n}")),
    };
    let high_stakes = match v["stakes"].as_str() {
        None | Some("normal") => false,
        Some("high") => true,
        Some(_) => return Err(Fail::input(&p("stakes"), "stakes is normal or high")),
    };
    if high_stakes && rule != PassRule::AllOfK {
        return Err(Fail::input(&p("pass_rule"), "a high-stakes case passes all of k"));
    }
    let overrides = match v.get("overrides") {
        None | Some(Value::Null) => Value::Null,
        Some(o) => {
            replays::parse_overrides(Some(o)).map_err(|f| Fail::input(&p("overrides"), f.message))?;
            o.clone()
        }
    };
    Ok(Case { id, source, graders: gs.clone(), k, rule, high_stakes, overrides, raw: v.clone() })
}

/// Validate a suite document.
pub fn parse_suite(v: &Value) -> Result<Suite, Fail> {
    let o = v.as_object().ok_or_else(|| Fail::input("suite", "a suite is an object"))?;
    if let Some(x) = o.keys().find(|x| !matches!(x.as_str(), "schema" | "id" | "name" | "description" | "cases")) {
        return Err(Fail::unsupported(x, format!("unknown suite field `{x}`")));
    }
    if v["schema"] != SUITE_SCHEMA {
        return Err(Fail::input("schema", format!("schema must be {SUITE_SCHEMA}")));
    }
    let id = match &v["id"] {
        Value::Null => new_id("suite"),
        Value::String(s) if valid_id(s) => s.clone(),
        _ => return Err(Fail::input("id", "suite id must be 1-64 characters of [A-Za-z0-9_.-]")),
    };
    let name = v["name"].as_str().filter(|n| !n.trim().is_empty() && n.len() <= 200).ok_or_else(|| Fail::input("name", "name must be 1-200 characters"))?.to_string();
    let cases = v["cases"].as_array().filter(|a| a.len() <= MAX_CASES).ok_or_else(|| Fail::input("cases", format!("cases must list 0-{MAX_CASES} cases")))?;
    let cases = cases.iter().enumerate().map(|(i, c)| parse_case(c, i)).collect::<Result<Vec<_>, _>>()?;
    let mut ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        return Err(Fail::input("cases", "case ids must be unique within a suite"));
    }
    let mut raw = v.clone();
    raw["id"] = json!(id);
    Ok(Suite { id, name, cases, raw })
}

/// A shipped task-type eval set as a suite: `expect` becomes graders.
pub fn shipped_suite(raw: &str) -> Suite {
    let set: Value = serde_json::from_str(raw).expect("shipped eval sets are valid JSON");
    let tt = set["task_type"].as_str().unwrap_or_default().to_string();
    let cases = set["cases"].as_array().cloned().unwrap_or_default().into_iter().map(|c| {
        let exp = &c["expect"];
        let mut gs = vec![json!({ "type": "status", "equals": exp["status"] })];
        if let Some(s) = exp["stopped_at"].as_str() {
            gs.push(json!({ "type": "stopped_at", "step": s }));
        }
        if exp["visited"].is_array() {
            gs.push(json!({ "type": "required_steps", "steps": exp["visited"] }));
        }
        if exp["not_visited"].is_array() {
            gs.push(json!({ "type": "forbidden_step", "steps": exp["not_visited"] }));
        }
        if exp["missing"].is_array() {
            gs.push(json!({ "type": "missing_evidence", "equals": exp["missing"] }));
        }
        let fixture = Fixture { task_type: tt.clone(), goal: c["goal"].as_str().unwrap_or_default().to_string(),
            params: c.get("params").cloned().unwrap_or_else(|| json!({})), script: c.get("script").cloned().unwrap_or_else(|| json!({})) };
        Case { id: c["id"].as_str().unwrap_or_default().to_string(), source: Source::Scripted(fixture), graders: gs, k: 1,
            rule: PassRule::AllOfK, high_stakes: false, overrides: Value::Null, raw: c.clone() }
    }).collect();
    Suite { id: format!("task_types.{tt}"), name: format!("Shipped eval set: {tt}"), cases, raw: set }
}

// ── trajectories ────────────────────────────────────────────────────────────

/// Walk a fixture with the scripted runner.
pub fn scripted_trajectory(f: &Fixture) -> Trajectory {
    let script: runner::Script = serde_json::from_value(f.script.clone()).unwrap_or_default();
    let out = runner::run_scripted(&f.task_type, &f.goal, &f.params, &script);
    let writable: Vec<String> = f.params["writable_resources"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
    let graph = task_types::get(&f.task_type).and_then(|t| task_types::instantiate(t, &format!("task.{}.eval", t.id.to_ascii_lowercase()), &writable).ok());
    let node = |id: &str| graph.as_ref().and_then(|g| g.node(id).cloned());
    let steps: Vec<Step> = out.visited.iter().map(|(n, ok)| Step { node: n.clone(), primitive: node(n).map(|x| x.primitive_id).unwrap_or_default(), ok: *ok }).collect();
    let effects = out.visited.iter().filter_map(|(n, ok)| {
        let g = node(n)?;
        (!g.write_set.is_empty()).then(|| Effect { node: n.clone(), tool: if g.primitive_id == "mut.create_file" { "tool.write_file" } else { "tool.execute" }.into(), committed: *ok })
    }).collect();
    let status = out.status.as_str().to_string();
    let verifier = match out.status {
        runner::Status::Completed => Some(true),
        runner::Status::NeedsEvidence => Some(false),
        _ => None,
    };
    Trajectory { status, verifier, steps, effects, texts: vec![], stopped_at: out.stopped_at.clone(), missing: out.missing.clone(), ..Default::default() }
}

/// The trajectory of a recorded run: its progress events, effect journal,
/// verification and usage.
pub fn run_trajectory(rec: &RunRecord, events: &[Value], journal: &[monitor::JournalRow], texts: Vec<String>) -> Trajectory {
    let steps = events.iter().filter(|e| e["type"] == "run.progress").map(|e| {
        let d = &e["data"];
        Step { node: d["step"].as_str().unwrap_or_default().to_string(), primitive: d["primitive_id"].as_str().unwrap_or_default().to_string(),
               ok: d["outcome"] == "committed" }
    }).collect();
    let effects = journal.iter().filter(|r| r.tool != "model.generate")
        .map(|r| Effect { node: r.node.clone(), tool: r.tool.clone(), committed: r.status == "committed" }).collect();
    let last_verify = events.iter().rev().find(|e| e["type"] == "verification.completed");
    let u = &rec.run["budget_usage"];
    Trajectory {
        status: rec.run["status"].as_str().unwrap_or_default().to_string(),
        verifier: last_verify.map(|e| e["data"]["result"] == "pass"),
        steps,
        effects,
        texts,
        stopped_at: events.iter().rev().find(|e| e["type"] == "run.progress").and_then(|e| e["data"]["step"].as_str().map(String::from)),
        missing: last_verify.and_then(|e| e["data"]["missing"].as_array()).map(|m| m.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default(),
        cost_usd: u["cost_usd"].as_f64().unwrap_or(0.0),
        tokens: u["tokens"].as_u64().unwrap_or(0),
        latency_ms: u["duration_ms"].as_f64().unwrap_or(0.0),
    }
}

/// The trajectory of a replay of `source` (its `replay_results`). Effects
/// are the source's, served from the recording where the replay committed
/// the node; texts are the recorded outputs.
pub fn replay_trajectory(results: &Value, source: &Trajectory) -> Trajectory {
    let nodes = results["nodes"].as_array().cloned().unwrap_or_default();
    let primitive = |n: &str| source.steps.iter().find(|s| s.node == n).map(|s| s.primitive.clone()).unwrap_or_default();
    let steps: Vec<Step> = nodes.iter().map(|n| {
        let id = n["node_id"].as_str().unwrap_or_default();
        Step { node: id.to_string(), primitive: primitive(id), ok: n["outcome"] == "committed" }
    }).collect();
    let committed = |node: &str| steps.iter().any(|s| s.node == node && s.ok);
    let effects = source.effects.iter().filter(|e| committed(&e.node)).cloned().collect();
    let m = &results["metrics"]["run"];
    let status = match m["status"]["replay"].as_str() {
        Some("recorded") | None => source.status.clone(),
        Some(s) => s.to_string(),
    };
    Trajectory {
        status,
        verifier: m["verifier"]["replay"].as_str().map(|v| v == "pass"),
        stopped_at: steps.last().map(|s| s.node.clone()),
        steps,
        effects,
        texts: source.texts.clone(),
        missing: source.missing.clone(),
        cost_usd: m["cost_usd"]["replay"].as_f64().unwrap_or(source.cost_usd),
        tokens: m["tokens"]["replay"].as_u64().unwrap_or(source.tokens),
        latency_ms: m["latency_ms"]["replay"].as_f64().unwrap_or(source.latency_ms),
    }
}

/// Recorded model outputs and document contents of a run.
fn run_texts(st: &AppState, run_id: &str, events: &[Value]) -> Vec<String> {
    let mut texts: Vec<String> = events.iter().filter(|e| e["type"] == "artifact.created").filter_map(|e| e["data"]["content"].as_str().map(String::from)).collect();
    if let Ok(c) = st.db.connect() {
        if let Ok(mut q) = c.prepare("SELECT result FROM agency_effect_journal WHERE run_id = ?1 AND tool = 'model.generate' AND status = 'committed' ORDER BY created_at, rowid") {
            let rows = q.query_map([run_id], |r| r.get::<_, Option<String>>(0)).map(|r| r.flatten().flatten().collect::<Vec<_>>()).unwrap_or_default();
            texts.extend(rows.into_iter().map(|s| serde_json::from_str::<Value>(&s).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or(s)));
        }
    }
    texts
}

async fn source_trajectory(st: &AppState, rec: &RunRecord) -> Trajectory {
    let run_id = rec.run["id"].as_str().unwrap_or_default();
    let events = store(st).events(run_id).await.unwrap_or_default();
    let journal = monitor::load_journal(&st.db, run_id).unwrap_or_default();
    let texts = run_texts(st, run_id, &events);
    run_trajectory(rec, &events, &journal, texts)
}

// ── running cases ───────────────────────────────────────────────────────────

/// Where cases run: offline (scripted only) or on a server (replays too).
pub enum Ctx<'a> {
    Offline,
    Online { st: &'a Arc<AppState>, user: &'a AuthUser, admin: bool, overrides: &'a Value },
}

fn attempt_json(i: u32, pass: bool, grades: &[graders::Grade]) -> Value {
    json!({ "attempt": i + 1, "pass": pass, "grades": grades.iter().map(|g| g.json()).collect::<Vec<_>>() })
}

fn graders_of(case: &Case) -> Vec<Box<dyn graders::Grader>> {
    case.graders.iter().filter_map(|g| graders::parse(g).ok()).collect()
}

fn run_scripted_case(case: &Case, f: &Fixture, via: &str) -> Value {
    let gs = graders_of(case);
    let mut attempts = vec![];
    let mut passes = 0;
    for i in 0..case.k {
        let t = scripted_trajectory(f);
        let (pass, grades) = graders::grade_all(&gs, &t);
        passes += pass as u32;
        attempts.push(attempt_json(i, pass, &grades));
    }
    case_result(case, via, attempts, passes, None)
}

fn case_result(case: &Case, via: &str, attempts: Vec<Value>, passes: u32, error: Option<Value>) -> Value {
    let pass = error.is_none() && case.rule.passes(passes, case.k);
    let (src, tt) = match &case.source {
        Source::Run { run_id, fixture } => (Some(run_id.clone()), fixture.as_ref().map(|f| f.task_type.clone())),
        Source::Scripted(f) => (None, Some(f.task_type.clone())),
    };
    json!({ "case_id": case.id, "via": via, "source_run_id": src, "task_type": tt, "k": case.k, "pass_rule": case.rule.json(),
            "stakes": if case.high_stakes { "high" } else { "normal" }, "attempts": attempts, "passes": passes,
            "pass_rate": stats::round4(passes as f64 / case.k as f64), "pass": pass, "skipped": false, "error": error })
}

fn skipped(case: &Case, reason: Value) -> Value {
    json!({ "case_id": case.id, "k": case.k, "pass_rule": case.rule.json(), "stakes": if case.high_stakes { "high" } else { "normal" },
            "skipped": true, "skip_reason": reason, "pass": false, "attempts": [] })
}

/// The overrides a replayed case runs with: the case's, then the eval run's on top.
fn merged_overrides(case: &Value, run: &Value) -> Value {
    let mut m: Map<String, Value> = case.as_object().cloned().unwrap_or_default();
    if let Some(r) = run.as_object() {
        m.extend(r.clone());
    }
    if m.is_empty() { Value::Null } else { Value::Object(m) }
}

/// Run one case. Never fails: a case that cannot run is skipped or errors.
pub async fn run_case(ctx: &Ctx<'_>, case: &Case) -> Value {
    let (run_id, fixture) = match &case.source {
        Source::Scripted(f) => return run_scripted_case(case, f, "scripted"),
        Source::Run { run_id, fixture } => (run_id, fixture),
    };
    let Ctx::Online { st, user, admin, overrides } = ctx else {
        return match fixture {
            Some(f) => run_scripted_case(case, f, "scripted_fixture"),
            None => skipped(case, json!({ "code": "ERR_EVAL_OFFLINE", "message": "a replayed case needs the recorded run (offline: give the case a fixture)" })),
        };
    };
    let src = store(st).load_run(run_id).await.ok().flatten().filter(|r| r.owner == user.user_id || *admin);
    let Some(src) = src else {
        return match fixture {
            Some(f) => run_scripted_case(case, f, "scripted_fixture"),
            None => skipped(case, json!({ "code": "ERR_NOT_FOUND", "message": "source run not found" })),
        };
    };
    if !replays::FINISHED.contains(&src.run["status"].as_str().unwrap_or_default()) {
        return skipped(case, json!({ "code": "ERR_REPLAY_NOT_FINISHED", "message": format!("run is {}", src.run["status"].as_str().unwrap_or("unknown")) }));
    }
    let ov = match replays::parse_overrides(Some(&merged_overrides(&case.overrides, overrides))) {
        Ok(o) => o,
        Err(f) => return case_result(case, "replay", vec![], 0, Some(f.json())),
    };
    let source = source_trajectory(st, &src).await;
    let gs = graders_of(case);
    let (mut attempts, mut passes) = (vec![], 0);
    for i in 0..case.k {
        let rpl = new_id("rpl");
        let r = replays::replay_object(&rpl, run_id, &ov, None);
        let out = replays::replay_one(st, &src, &ov, &rpl, None).await;
        let done = replays::settle(r, out.as_ref());
        if let Err(e) = store(st).save_object(EV_REPLAY_STATE, &rpl, &user.user_id, &done).await {
            tracing::warn!(replay = %rpl, error = %e, "eval replay not stored");
        }
        match out {
            Ok(o) => {
                let t = replay_trajectory(&o.results, &source);
                let (pass, grades) = graders::grade_all(&gs, &t);
                passes += pass as u32;
                let mut a = attempt_json(i, pass, &grades);
                a["replay_id"] = json!(rpl);
                a["verdict"] = json!(o.verdict);
                attempts.push(a);
            }
            Err(f) => {
                let stop = f.code == "ERR_BUDGET_CAP_REACHED";
                attempts.push(json!({ "attempt": i + 1, "pass": false, "replay_id": rpl, "error": f.json() }));
                if stop {
                    // Fail closed: no more live calls once a daily cap is reached.
                    return case_result(case, "replay", attempts, passes, Some(f.json()));
                }
            }
        }
    }
    case_result(case, "replay", attempts, passes, None)
}

/// Aggregate metrics of a suite's case results.
pub fn suite_metrics(cases: &[Value], seed: u64, resamples: usize) -> Value {
    let graded: Vec<&Value> = cases.iter().filter(|c| c["skipped"] != true).collect();
    let outcomes: Vec<bool> = graded.iter().map(|c| c["pass"] == true).collect();
    let (att_pass, att_total) = graded.iter().fold((0u64, 0u64), |(p, n), c| (p + c["passes"].as_u64().unwrap_or(0), n + c["k"].as_u64().unwrap_or(0)));
    let ci = stats::bootstrap_ci95(&outcomes, seed, resamples);
    let high: Vec<&Value> = graded.iter().filter(|c| c["stakes"] == "high" && c["pass"] != true).map(|c| &c["case_id"]).collect();
    json!({
        "cases": cases.len(), "graded": graded.len(), "skipped": cases.len() - graded.len(),
        "passed": outcomes.iter().filter(|x| **x).count(),
        "pass_rate": stats::rate(&outcomes).map(stats::round4),
        "attempt_pass_rate": (att_total > 0).then(|| stats::round4(att_pass as f64 / att_total as f64)),
        "ci95": ci.map(|(l, h)| json!({ "low": l, "high": h, "method": "percentile_bootstrap", "resamples": resamples.clamp(100, stats::MAX_RESAMPLES), "seed": seed })),
        "high_stakes_failures": high,
    })
}

pub async fn run_suite(ctx: &Ctx<'_>, suite: &Suite, seed: u64, resamples: usize) -> Value {
    let mut cases = Vec::with_capacity(suite.cases.len());
    for c in &suite.cases {
        cases.push(run_case(ctx, c).await);
    }
    let metrics = suite_metrics(&cases, seed, resamples);
    json!({ "suite_id": suite.id, "name": suite.name, "cases": cases, "metrics": metrics })
}

/// Every shipped eval set and committed suite, scripted (the CI gate's input).
pub async fn offline_report(seed: u64) -> Value {
    let mut suites = Map::new();
    let all: Vec<Suite> = SHIPPED_EVALS.iter().map(|r| shipped_suite(r))
        .chain(COMMITTED_SUITES.iter().map(|r| parse_suite(&serde_json::from_str(r).expect("committed suites are valid JSON")).expect("committed suites validate")))
        .collect();
    for s in &all {
        let r = run_suite(&Ctx::Offline, s, seed, stats::DEFAULT_RESAMPLES).await;
        let failed: Vec<Value> = r["cases"].as_array().into_iter().flatten().filter(|c| c["skipped"] != true && c["pass"] != true).map(|c| c["case_id"].clone()).collect();
        let mut m = r["metrics"].clone();
        m["failed_cases"] = json!(failed);
        suites.insert(s.id.clone(), m);
    }
    json!({ "schema": REPORT_SCHEMA, "seed": seed, "generated_by": "agency_api::evals::offline_report", "suites": suites })
}

// ── handlers ────────────────────────────────────────────────────────────────

fn is_admin(user: &AuthUser) -> bool {
    flywheel::is_flywheel_admin(&user.user_id)
}

fn suite_object(s: &Suite, owner: &str, created_at: &str) -> Value {
    let mut o = s.raw.clone();
    o["object"] = json!("eval_suite");
    o["owner"] = json!(owner);
    o["case_count"] = json!(s.cases.len());
    o["updated_at"] = json!(now());
    o["created_at"] = json!(created_at);
    o
}

fn stored_suite(v: &Value) -> Option<Suite> {
    let mut raw = v.clone();
    if let Some(o) = raw.as_object_mut() {
        for k in ["object", "owner", "case_count", "updated_at", "created_at"] {
            o.remove(k);
        }
    }
    parse_suite(&raw).ok()
}

/// `POST /v1/evals/suites`: create or replace one of the caller's suites.
pub async fn create_suite(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<Value>,
) -> ApiResult {
    let suite = parse_suite(&body).map_err(|f| f.api(&rid))?;
    let s = store(&st);
    let prev = s.load_object(EV_EVAL_SUITE_STATE, &suite.id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?;
    let created = prev.as_ref().and_then(|p| p["created_at"].as_str().map(String::from)).unwrap_or_else(now);
    let o = suite_object(&suite, &user.user_id, &created);
    s.save_object(EV_EVAL_SUITE_STATE, &suite.id, &user.user_id, &o).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((if prev.is_some() { StatusCode::OK } else { StatusCode::CREATED }, Json(o)).into_response())
}

pub async fn list_suites(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Extension(rid): Extension<RequestId>) -> ApiResult {
    let v = store(&st).list_objects(EV_EVAL_SUITE_STATE, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(super::page(v)).into_response())
}

pub async fn get_suite(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let v = store(&st).load_object(EV_EVAL_SUITE_STATE, &id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("eval suite", &rid))?;
    Ok(Json(v).into_response())
}

/// `POST /v1/evals/runs`: run a stored suite (`suite_id`) or an inline one
/// (`suite`), and/or the shipped task-type sets (`include_shipped`).
pub async fn create_eval_run(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<Value>,
) -> ApiResult {
    let obj = body.as_object().ok_or_else(|| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "body must be an object", &rid))?;
    if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "suite_id" | "suite" | "include_shipped" | "seed" | "resamples" | "overrides" | "effects")) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", format!("unknown field `{k}`"), &rid).param(Some(k.clone())));
    }
    replays::check_effects(obj).map_err(|f| f.api(&rid))?;
    let overrides = obj.get("overrides").cloned().unwrap_or(Value::Null);
    let ov = replays::parse_overrides(Some(&overrides)).map_err(|f| f.api(&rid))?;
    let seed = match obj.get("seed") {
        None | Some(Value::Null) => stats::DEFAULT_SEED,
        Some(v) => v.as_u64().ok_or_else(|| Fail::input("seed", "seed must be a non-negative integer").api(&rid))?,
    };
    let resamples = match obj.get("resamples") {
        None | Some(Value::Null) => stats::DEFAULT_RESAMPLES,
        Some(v) => v.as_u64().filter(|n| (100..=stats::MAX_RESAMPLES as u64).contains(n))
            .ok_or_else(|| Fail::input("resamples", format!("resamples must be 100-{}", stats::MAX_RESAMPLES)).api(&rid))? as usize,
    };
    let s = store(&st);
    let mut suites: Vec<Suite> = vec![];
    match (obj.get("suite_id"), obj.get("suite")) {
        (Some(_), Some(_)) => return Err(Fail::input("suite_id", "give suite_id or suite, not both").api(&rid)),
        (Some(id), None) => {
            let id = id.as_str().unwrap_or_default();
            let v = s.load_object(EV_EVAL_SUITE_STATE, id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
                .ok_or_else(|| ApiError::not_found("eval suite", &rid))?;
            suites.push(stored_suite(&v).ok_or_else(|| ApiError::internal("stored suite no longer validates", &rid))?);
        }
        (None, Some(inline)) => suites.push(parse_suite(inline).map_err(|f| f.api(&rid))?),
        (None, None) => {}
    }
    if obj.get("include_shipped").and_then(Value::as_bool).unwrap_or(false) {
        suites.extend(SHIPPED_EVALS.iter().map(|r| shipped_suite(r)));
    }
    if suites.is_empty() {
        return Err(Fail::input("suite_id", "give suite_id, suite or include_shipped: true").api(&rid));
    }
    let id = new_id("evr");
    let run = json!({ "id": id, "object": "eval_run", "status": "running", "effects": "recorded_only", "overrides": ov.echo(),
        "seed": seed, "resamples": resamples, "suite_ids": suites.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        "created_at": now(), "finished_at": null, "suites": [], "metrics": null, "error": null });
    // Live model calls (a cognition override outside scripted mode) run in the background.
    let live = suites.iter().any(|su| su.cases.iter().any(|c| matches!(c.source, Source::Run { .. })
        && replays::parse_overrides(Some(&merged_overrides(&c.overrides, &overrides))).map(|o| replays::runs_live(&o)).unwrap_or(false)));
    let admin = is_admin(&user);
    if live {
        let run_shown = run.clone();
        s.save_object(EV_EVAL_RUN_STATE, &id, &user.user_id, &run).await.map_err(|e| ApiError::internal(e, &rid))?;
        let st2 = st.clone();
        tokio::spawn(async move {
            let done = execute(&st2, &user, admin, &overrides, suites, run, seed, resamples).await;
            let id = done["id"].as_str().unwrap_or_default().to_string();
            if let Err(e) = store(&st2).save_object(EV_EVAL_RUN_STATE, &id, &user.user_id, &done).await {
                tracing::warn!(eval_run = %id, error = %e, "eval run not stored");
            }
        });
        return Ok((StatusCode::ACCEPTED, Json(run_shown)).into_response());
    }
    let done = execute(&st, &user, admin, &overrides, suites, run, seed, resamples).await;
    s.save_object(EV_EVAL_RUN_STATE, &id, &user.user_id, &done).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::ACCEPTED, Json(done)).into_response())
}

#[allow(clippy::too_many_arguments)]
async fn execute(st: &Arc<AppState>, user: &AuthUser, admin: bool, overrides: &Value, suites: Vec<Suite>, mut run: Value, seed: u64, resamples: usize) -> Value {
    let ctx = Ctx::Online { st, user, admin, overrides };
    let mut results = vec![];
    let mut all = vec![];
    for s in &suites {
        let r = run_suite(&ctx, s, seed, resamples).await;
        all.extend(r["cases"].as_array().cloned().unwrap_or_default());
        results.push(r);
    }
    run["status"] = json!("completed");
    run["finished_at"] = json!(now());
    run["suites"] = json!(results);
    run["metrics"] = suite_metrics(&all, seed, resamples);
    run
}

pub async fn get_eval_run(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let v = store(&st).load_object(EV_EVAL_RUN_STATE, &id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("eval run", &rid))?;
    Ok(Json(v).into_response())
}

/// A regression case snapshotted from a finished run: ids, path, effect
/// count, verdict, findings and a scripted fixture. No goal text (only its
/// hash), no outputs, no credentials.
pub fn case_from_record(rec: &RunRecord, events: &[Value], source: &Trajectory, findings: &Value, body: &Value) -> Result<Value, Fail> {
    let run_id = rec.run["id"].as_str().unwrap_or_default();
    let ir = &rec.task_ir;
    let tt = ir["task_type"].as_str().unwrap_or("BUG_FIX").to_string();
    let write_set: Vec<String> = ir["wih_policy"]["write_set"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
    let mut path: Vec<String> = source.steps.iter().filter(|s| s.ok).map(|s| s.node.clone()).collect();
    path.dedup();
    let effects = source.effects.iter().filter(|e| e.committed).count();
    let checks: Vec<Value> = findings["data"].as_array().into_iter().flatten().map(|f| json!({ "check": f["check"], "severity": f["severity"] })).collect();
    let severe = findings["data"].as_array().into_iter().flatten().any(|f| matches!(f["severity"].as_str(), Some("high" | "critical")));
    // Scripted fixture (task types only): WAIT answers and failed steps as recorded.
    let fixture = (tt != "BUG_FIX" && task_types::get(&tt).is_some()).then(|| {
        let wait: Map<String, Value> = rec.attention.iter().filter(|a| a["reason"] == TASK_WAIT_REASON && a["status"] == "resolved")
            .filter_map(|a| Some((a["node_id"].as_str()?.to_string(), json!(if a["resolution"]["outcome"] == "approved" { "open" } else { "closed" }))))
            .collect();
        let mut fail: Vec<String> = events.iter().filter(|e| e["type"] == "run.progress" && e["data"]["outcome"] == "failed")
            .filter_map(|e| e["data"]["step"].as_str().map(String::from)).collect();
        fail.sort();
        fail.dedup();
        json!({ "task_type": tt, "params": { "writable_resources": write_set }, "script": { "wait": wait, "fail": fail } })
    });
    let mut graders_v = vec![json!({ "type": "verifier_pass" }), json!({ "type": "effect_count", "max": effects })];
    if source.verifier == Some(true) && !path.is_empty() {
        graders_v.push(json!({ "type": "step_order", "steps": path.iter().take(64).collect::<Vec<_>>() }));
    }
    if source.cost_usd > 0.0 {
        graders_v.push(json!({ "type": "cost_ceiling", "max_usd": stats::round4(source.cost_usd * 2.0) }));
    }
    let id = match body.get("case_id") {
        None | Some(Value::Null) => format!("reg_{}", run_id.trim_start_matches("run_")).chars().take(64).collect(),
        Some(Value::String(s)) if valid_id(s) => s.clone(),
        Some(_) => return Err(Fail::input("case_id", "case_id must be 1-64 characters of [A-Za-z0-9_.-]")),
    };
    let mut case = json!({
        "id": id, "title": format!("Regression from {run_id}"), "source_run_id": run_id,
        "graders": body.get("graders").cloned().unwrap_or_else(|| json!(graders_v)),
        "k": body.get("k").cloned().unwrap_or(json!(1)),
        "pass_rule": body.get("pass_rule").cloned().unwrap_or(json!("all_of_k")),
        "stakes": body.get("stakes").cloned().unwrap_or(json!(if severe { "high" } else { "normal" })),
        "snapshot": {
            "run_id": run_id, "task_type": tt, "graph_id": ir["dag_id"], "template_id": ir["template"]["id"], "write_set": write_set,
            "status": rec.run["status"], "verifier": source.verifier.map(|v| if v { "pass" } else { "fail" }), "path": path,
            "effects": effects, "cost_usd": source.cost_usd, "tokens": source.tokens, "latency_ms": source.latency_ms,
            "goal_sha256": allternit_factory_engine::receipts::jcs::sha256_tagged(rec.run["goal"].as_str().unwrap_or_default().as_bytes()),
            "findings": checks, "promoted_at": now(),
        },
    });
    if let Some(o) = body.get("overrides").filter(|o| !o.is_null()) {
        case["overrides"] = o.clone();
    }
    if let Some(f) = fixture {
        case["fixture"] = f;
    }
    parse_case(&case, 0).map_err(|f| Fail { param: f.param.map(|p| p.trim_start_matches("cases[0].").to_string()), ..f })?;
    Ok(case)
}

/// `POST /v1/evals/cases/from-run/:run_id` (owner): promote a finished run
/// into a suite (default `regressions`) as a regression case.
pub async fn case_from_run(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    let body: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(&body).map_err(|e| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", format!("invalid JSON: {e}"), &rid))?
    };
    let obj = body.as_object().ok_or_else(|| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "body must be an object", &rid))?;
    if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "suite_id" | "case_id" | "graders" | "k" | "pass_rule" | "stakes" | "overrides")) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", format!("unknown field `{k}`"), &rid).param(Some(k.clone())));
    }
    let s = store(&st);
    let rec = owned(s.load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    if !replays::FINISHED.contains(&rec.run["status"].as_str().unwrap_or_default()) {
        return Err(ApiError::new(409, "STATE", "ERR_RUN_NOT_FINISHED", format!("run is {}; only finished runs become eval cases",
            rec.run["status"].as_str().unwrap_or("unknown")), &rid));
    }
    let suite_id = match obj.get("suite_id") {
        None | Some(Value::Null) => DEFAULT_REGRESSION_SUITE.to_string(),
        Some(Value::String(s)) if valid_id(s) => s.clone(),
        Some(_) => return Err(Fail::input("suite_id", "suite_id must be 1-64 characters of [A-Za-z0-9_.-]").api(&rid)),
    };
    let findings = match monitor::stored(&s, &run_id, &rec.owner).await.map_err(|e| ApiError::internal(e, &rid))? {
        Some(f) => f,
        None => monitor::monitor_run(&st, &run_id, false).await.map_err(|e| ApiError::internal(e, &rid))?.unwrap_or(Value::Null),
    };
    let events = s.events(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?;
    let source = source_trajectory(&st, &rec).await;
    let case = case_from_record(&rec, &events, &source, &findings, &body).map_err(|f| f.api(&rid))?;
    let _g = s.lock().await;
    let prev = s.load_object(EV_EVAL_SUITE_STATE, &suite_id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?;
    let created = prev.as_ref().and_then(|p| p["created_at"].as_str().map(String::from)).unwrap_or_else(now);
    let mut raw = match &prev {
        Some(p) => stored_suite(p).map(|su| su.raw).ok_or_else(|| ApiError::internal("stored suite no longer validates", &rid))?,
        None => json!({ "schema": SUITE_SCHEMA, "id": suite_id, "name": "Regressions from production runs", "cases": [] }),
    };
    let cases = raw["cases"].as_array_mut().ok_or_else(|| ApiError::internal("suite has no cases", &rid))?;
    cases.retain(|c| c["id"] != case["id"]);
    cases.push(case.clone());
    let suite = parse_suite(&raw).map_err(|f| f.api(&rid))?;
    let o = suite_object(&suite, &user.user_id, &created);
    s.save_object(EV_EVAL_SUITE_STATE, &suite_id, &user.user_id, &o).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::CREATED, Json(json!({ "object": "eval_case", "suite_id": suite_id, "case": case, "suite_case_count": suite.cases.len() }))).into_response())
}
