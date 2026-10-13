//! B1: live S1 decision nodes in task-type graphs.
//!
//! A node with `cognitive_role: "S1"` is a choice, not text generation. Its
//! decision spec lives in the ABI extension map (`extensions["x-decision"]`):
//!
//! ```json
//! { "kind": "verify", "question": "Does the result meet the goal?",
//!   "options": [{ "id": "pass", "text": "..." }, { "id": "fail", "text": "..." }],
//!   "options_from": "candidate:routes", "pass": ["pass"], "routes": { "fail": "G07" },
//!   "min_confidence": 0.8 }
//! ```
//! * `options` is a static list; `options_from` takes them from a prior output
//!   instead (a JSON array of strings or `{id, text}`, else one per line; ids
//!   `o1..` for bare strings). Exactly one option is chosen without asking.
//! * Routing: a chosen id named in `routes` commits the node and goes to that
//!   node (it needs an out-edge there; plain walks skip routed edges). This
//!   is the ABI edge `when` selector, kept in the node spec; else a chosen id in `pass` (default: every option) commits
//!   and takes the NORMAL edge; any other choice is a confident rejection and
//!   fails the run without escalation (a decided "fail" is a verdict).
//! * Abstain, confidence under the kind's threshold (or `min_confidence`), no
//!   backend answering, or an unusable spec: the node fails and its on_failure
//!   ESCALATE edge runs the S2/S3 twin (`x-twin_of`), like tier escalation.
//! * The chosen id is written to every declared output.
//!
//! Determinism: the Decision Runtime answers by argmax over per-option
//! probabilities (no sampling; ties go to the earlier option) and the answer
//! is journaled per (run, node) (`journal_key`) before it is acted on, so a
//! re-drive or replay serves the journaled answer and never decides again,
//! even when the backend chain or its latency would answer differently now.

use crate::db::DbHandle;
use allternit_factory_engine::kernel::graph::{ComputeGraph, GraphNode};
use allternit_factory_engine::kernel::router::ExecutionPlan;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const SPEC_KEY: &str = "x-decision";
/// The run-scoped backend id S1 plans carry (the Decision Runtime chain).
pub const BACKEND_ID: &str = "decision-runtime";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Spec {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub options: Vec<Opt>,
    #[serde(default)]
    pub options_from: Option<String>,
    #[serde(default)]
    pub pass: Option<Vec<String>>,
    #[serde(default)]
    pub min_confidence: Option<f64>,
    /// Chosen option id -> the node it routes to (an out-edge of this node).
    #[serde(default)]
    pub routes: HashMap<String, String>,
    /// Backend chain override (names from `decisions::backends`).
    #[serde(default)]
    pub backends: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opt {
    pub id: String,
    #[serde(alias = "label")]
    pub text: String,
}

/// One answered decision, as journaled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The stored decision (`/v1/decisions/:id`); None in scripted mode.
    pub decision_id: Option<String>,
    pub choice: Option<String>,
    pub confidence: f64,
    pub abstained: bool,
    pub backend: String,
}

impl Answer {
    pub fn abstain(backend: &str) -> Self {
        Self { decision_id: None, choice: None, confidence: 0.0, abstained: true, backend: backend.into() }
    }
    /// From a `decide_value` reply; under `min_confidence` counts as abstaining.
    pub fn from_decision(v: &Value, min_confidence: Option<f64>) -> Self {
        let confidence = v["confidence"].as_f64().unwrap_or(0.0);
        let low = min_confidence.is_some_and(|m| confidence < m);
        let choice = v["choice"].as_str().filter(|_| !low && v["abstained"] != true).map(str::to_string);
        Self { decision_id: v["id"].as_str().map(str::to_string), abstained: choice.is_none(), choice, confidence,
            backend: v["backend"].as_str().unwrap_or("none").to_string() }
    }
}

pub fn is_s1(n: &GraphNode) -> bool {
    n.cognitive_role.as_deref() == Some("S1")
}

pub fn spec(n: &GraphNode) -> Result<Spec> {
    let v = n.extensions.as_ref().and_then(|e| e.get(SPEC_KEY)).ok_or_else(|| anyhow!("{}: S1 node without {SPEC_KEY}", n.node_id))?;
    Ok(serde_json::from_value(v.clone())?)
}

impl Spec {
    pub fn kind(&self) -> &str {
        self.kind.as_deref().unwrap_or("choice")
    }
    /// The options for this decision: static, or parsed from a prior output.
    pub fn options(&self, out: &HashMap<String, String>) -> Vec<Opt> {
        let Some(src) = &self.options_from else { return self.options.clone() };
        let raw = out.get(src).map(String::as_str).unwrap_or_default();
        let mut opts: Vec<Opt> = match serde_json::from_str::<Vec<Value>>(raw.trim()) {
            Ok(a) => a.iter().enumerate().filter_map(|(i, v)| match v {
                Value::String(s) => Some(Opt { id: format!("o{}", i + 1), text: s.trim().to_string() }),
                v => serde_json::from_value::<Opt>(v.clone()).ok(),
            }).collect(),
            Err(_) => raw.lines().map(str::trim).filter(|l| !l.is_empty()).enumerate()
                .map(|(i, l)| Opt { id: format!("o{}", i + 1), text: l.to_string() }).collect(),
        };
        let mut seen = std::collections::HashSet::new();
        opts.retain(|o| !o.text.is_empty() && seen.insert(o.id.clone()));
        opts
    }
}

/// Where the walk goes after an S1 node answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Commit; `Some(target)` is a routed edge, `None` the NORMAL edge.
    Commit(Option<String>),
    /// No usable answer: fail the node, follow its ESCALATE/FAILURE edge.
    Escalate,
    /// A confident choice outside `pass`: the run fails on it.
    Reject(String),
}

pub fn route(g: &ComputeGraph, node: &str, spec: &Spec, a: &Answer) -> Route {
    let Some(c) = a.choice.as_deref().filter(|_| !a.abstained) else { return Route::Escalate };
    if let Some(t) = spec.routes.get(c).filter(|t| g.edges.iter().any(|e| e.from == node && &e.to == *t)) {
        return Route::Commit(Some(t.clone()));
    }
    match &spec.pass {
        Some(p) if !p.iter().any(|x| x == c) => Route::Reject(c.to_string()),
        _ => Route::Commit(None),
    }
}

/// The successor along `kind`, ignoring an S1 node's routed (decision-selected) edges.
pub fn next(g: &ComputeGraph, from: &str, kind: &str) -> Option<String> {
    let routed: Vec<String> = g.node(from).filter(|n| is_s1(n)).and_then(|n| spec(n).ok()).map(|s| s.routes.into_values().collect()).unwrap_or_default();
    g.edges.iter().find(|e| e.from == from && e.kind() == kind && !routed.contains(&e.to)).map(|e| e.to.clone())
}

/// Scripted answer: the script names an option id (or `abstain`) per node;
/// default = the first option.
pub fn scripted(opts: &[Opt], named: Option<&str>, failed: bool) -> Answer {
    let pick = match named {
        _ if failed => None,
        Some("abstain") => None,
        Some(id) => opts.iter().find(|o| o.id == id).map(|o| o.id.clone()),
        None => opts.first().map(|o| o.id.clone()),
    };
    Answer { decision_id: None, abstained: pick.is_none(), confidence: if pick.is_some() { 1.0 } else { 0.0 }, choice: pick, backend: "scripted".into() }
}

/// The plan an S1 node records (the router's S1 path needs a calibrated pool
/// entry; live S1 goes to the Decision Runtime instead).
pub fn plan(node: &GraphNode, spec: &Spec) -> ExecutionPlan {
    let cap = node.capability_request.as_ref().and_then(|c| c["capability"].as_str()).map(str::to_string)
        .unwrap_or_else(|| format!("cap.decide.{}", spec.kind()));
    serde_json::from_value(json!({
        "schema_id": "allternit.kernel.ExecutionPlanV1", "schema_version": "1.0.0", "plan_id": format!("s1:{}", node.node_id),
        "node_id": node.node_id, "cognitive_role": "S1", "capability_id": cap, "execution_mode": "M4.DEDICATED_DECIDER",
        "backend_id": BACKEND_ID, "confidence_floor": spec.min_confidence.unwrap_or(0.0), "fallback_chain": [] }))
        .expect("static S1 plan shape")
}

pub fn journal_key(run_id: &str, node: &str) -> String {
    format!("{run_id}:{node}:decision.decide:1")
}

/// Decide once per (run, node): a journaled answer is served as-is (`ask` is
/// not called); a fresh one is journaled before it is returned. `Ok(None)`:
/// this worker's fence is stale (stop without acting).
pub fn decide_journaled(db: &DbHandle, run_id: &str, node: &str, epoch: i64, ask: impl FnOnce() -> Answer) -> Result<Option<(Answer, bool)>> {
    let key = journal_key(run_id, node);
    if let Some(v) = crate::agency_api::safety::journaled_value(db, &key)? {
        if let Ok(a) = serde_json::from_value::<Answer>(v) {
            return Ok(Some((a, true)));
        }
    }
    let a = ask();
    if !crate::agency_api::safety::journal_value(db, &key, run_id, node, "decision.decide", epoch, &serde_json::to_value(&a)?)? {
        return Ok(None);
    }
    Ok(Some((a, false)))
}

/// Label a run's answered decisions with how the run ended: `success` (the
/// chosen option is the label) on verified completion, `failure` (no label)
/// on a verifier failure. Best effort: errors are logged, never returned.
pub fn label(st: &crate::AppState, user: &crate::auth::AuthUser, run_id: &str, decided: &[(String, String)], status: &str) -> usize {
    let detail = format!("agency run {run_id} {}", if status == "success" { "verified complete" } else { "failed verification" });
    decided.iter().filter(|(id, choice)| {
        let label = (status == "success").then_some(choice.as_str());
        crate::agency_api::decisions::record_outcome(st, user, id, status, label, Some(&detail))
            .map_err(|e| tracing::debug!(%run_id, decision = %id, error = %e, "S1 outcome not recorded")).is_ok()
    }).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn spec_json() -> Value {
        json!({ "kind": "verify", "pass": ["pass", "other"], "routes": { "other": "B" }, "options": [{ "id": "pass", "text": "ok" },
            { "id": "fail", "text": "no" }, { "id": "other", "label": "branch" }] })
    }

    fn graph() -> ComputeGraph {
        serde_json::from_value(json!({ "graph_id": "g", "entry_nodes": [], "completion_nodes": [],
            "nodes": [{ "node_id": "D", "primitive_id": "ver.verify_requirements", "node_kind": "VERIFY", "cognitive_role": "S1",
                "on_failure": { "strategy": "ESCALATE", "target": "X" }, "extensions": { "x-decision": spec_json() } }],
            "edges": [{ "from": "D", "to": "B" }, { "from": "D", "to": "N" }, { "from": "D", "to": "X", "edge_kind": "FAILURE" }] })).unwrap()
    }

    fn spec() -> Spec {
        serde_json::from_value(spec_json()).unwrap()
    }

    fn answer(choice: Option<&str>) -> Answer {
        Answer { decision_id: None, choice: choice.map(str::to_string), confidence: 0.9, abstained: choice.is_none(), backend: "t".into() }
    }

    #[test]
    fn choice_routes_commit_branch_reject_or_escalate() {
        let (g, sp) = (graph(), spec());
        assert_eq!(route(&g, "D", &sp, &answer(Some("pass"))), Route::Commit(None));
        assert_eq!(route(&g, "D", &sp, &answer(Some("other"))), Route::Commit(Some("B".into())), "a routed edge wins");
        assert_eq!(route(&g, "D", &sp, &answer(Some("fail"))), Route::Reject("fail".into()));
        assert_eq!(route(&g, "D", &sp, &answer(None)), Route::Escalate);
        // Plain walks never take a decision-selected edge.
        assert_eq!(next(&g, "D", "NORMAL").as_deref(), Some("N"));
        // Low confidence: the runtime's abstention, or under the node's floor.
        let low = Answer::from_decision(&json!({ "id": "dec_1", "choice": "pass", "confidence": 0.7, "abstained": false, "backend": "local" }), Some(0.8));
        assert_eq!((low.abstained, route(&g, "D", &sp, &low)), (true, Route::Escalate));
        let abst = Answer::from_decision(&json!({ "id": "dec_2", "choice": null, "confidence": 0.4, "abstained": true }), None);
        assert_eq!(route(&g, "D", &sp, &abst), Route::Escalate);
    }

    #[test]
    fn options_come_from_the_spec_or_a_prior_output() {
        let mut sp = spec();
        assert_eq!(sp.options(&HashMap::new()).len(), 3);
        sp.options_from = Some("candidate:routes".into());
        let out: HashMap<String, String> = [("candidate:routes".to_string(), "email\n\nsms\nemail".to_string())].into();
        let ids: Vec<String> = sp.options(&out).into_iter().map(|o| o.id).collect();
        assert_eq!(ids, ["o1", "o2", "o3"]);
        let out: HashMap<String, String> = [("candidate:routes".to_string(), r#"[{"id":"a","text":"A"},{"id":"a","text":"dup"},"c"]"#.to_string())].into();
        let ids: Vec<String> = sp.options(&out).into_iter().map(|o| o.id).collect();
        assert_eq!(ids, ["a", "o3"]);
        // Scripted: named option, abstain, default first, failed node.
        let o = spec().options;
        assert_eq!(scripted(&o, None, false).choice.as_deref(), Some("pass"));
        assert_eq!(scripted(&o, Some("fail"), false).choice.as_deref(), Some("fail"));
        assert!(scripted(&o, Some("abstain"), false).abstained);
        assert!(scripted(&o, None, true).abstained);
    }

    #[test]
    fn a_journaled_decision_is_never_decided_again() {
        let db = DbHandle::new_memory().unwrap();
        let e = crate::agency_api::safety::acquire_fence(&db, "run_s1", "w1").unwrap();
        let calls = Cell::new(0);
        let ask = |c: &str| { calls.set(calls.get() + 1); answer(Some(c)) };
        let (a, replayed) = decide_journaled(&db, "run_s1", "G05", e, || ask("pass")).unwrap().unwrap();
        assert_eq!((a.choice.as_deref(), replayed, calls.get()), (Some("pass"), false, 1));
        // Re-drive (new fence epoch): the journal answers, the runtime is not asked.
        let e2 = crate::agency_api::safety::acquire_fence(&db, "run_s1", "w2").unwrap();
        let (b, replayed) = decide_journaled(&db, "run_s1", "G05", e2, || ask("fail")).unwrap().unwrap();
        assert_eq!((b, replayed, calls.get()), (a, true, 1));
        // A stale worker does not get to journal (or act on) a fresh decision.
        assert!(decide_journaled(&db, "run_s1", "G06", e, || ask("pass")).unwrap().is_none());
    }

    #[tokio::test]
    async fn live_decision_escalates_when_nothing_answers_and_outcomes_label_it() {
        let t = crate::agency_api::tests::setup().await;
        let user = crate::auth::AuthUser { user_id: "u1".into(), email: None, name: None, avatar_url: None, tenant_id: None,
            organization_id: None, organization_role: None, organization_slug: None };
        let sp = spec();
        // No backend configured: the runtime abstains (stored, attributable to the run/node).
        let v = crate::agency_api::decisions::decide_value(&t.st, &user, json!({ "context": "Goal: g", "options": sp.options,
            "kind": "verify", "allow_abstain": true, "backends": ["local"], "session_id": "run_s1", "task": "run_s1:G05" })).await.unwrap();
        let a = Answer::from_decision(&v, None);
        assert_eq!(route(&graph(), "D", &sp, &a), Route::Escalate);
        let id = a.decision_id.clone().expect("the abstention is stored");
        let row = crate::agency_api::decisions::store::get(&t.st.db, &id, "u1").unwrap().unwrap();
        assert_eq!((row["session_id"].as_str(), row["task"].as_str()), (Some("run_s1"), Some("run_s1:G05")));
        // The run's verified completion labels its decisions.
        assert_eq!(label(&t.st, &user, "run_s1", &[(id.clone(), "pass".into()), ("dec_missing".into(), "pass".into())], "success"), 1);
        let row = crate::agency_api::decisions::store::get(&t.st.db, &id, "u1").unwrap().unwrap();
        assert_eq!(row["outcome"]["status"], "success", "{row}");
    }
}
