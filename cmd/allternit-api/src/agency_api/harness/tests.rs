//! C6: mutation operators, LLM patch validation, the acceptance rule, and
//! the propose -> approve -> pinned version -> rollback flow end to end.

use super::*;
use crate::agency_api::store::RunRecord;
use crate::agency_api::tests::{call, get_req, post, run_limits, setup, wait_settled, T, E2E_ENV};
use crate::agency_api::{executor, guard, store::AgencyStore};

fn tt(id: &str) -> &'static TaskType {
    task_types::get(id).unwrap()
}

/// GENERAL_TASK with its review as a plain S2 verification (no S1, no twin).
fn general_with_s2_review() -> Value {
    let mut g = shipped_graph(tt("GENERAL_TASK"));
    let n = g["nodes"].as_array_mut().unwrap().iter_mut().find(|n| n["node_id"] == "G05").unwrap();
    n["cognitive_role"] = json!("S2");
    n["on_failure"] = json!({ "strategy": "FAIL" });
    n["extensions"].as_object_mut().unwrap().remove("x-decision");
    g["nodes"].as_array_mut().unwrap().retain(|n| n["node_id"] != "G05_s2");
    g["edges"].as_array_mut().unwrap().retain(|e| e["from"] != "G05_s2" && e["to"] != "G05_s2");
    g
}

#[test]
fn harness_mutation_operators_produce_valid_graphs() {
    let mut by_op: BTreeMap<String, usize> = BTreeMap::new();
    for t in task_types::TASK_TYPES {
        let base = shipped_graph(t);
        mutate::validate(t, &base, &base).unwrap_or_else(|e| panic!("{}: shipped graph fails its own guardrails: {e}", t.id));
        for c in mutate::mutations(&base, &[]) {
            mutate::validate(t, &base, &c.graph).unwrap_or_else(|e| panic!("{} {} {:?}: {e}", t.id, c.operator, c.target));
            assert_ne!(c.graph, base, "{} is a no-op", c.operator);
            *by_op.entry(c.operator.clone()).or_default() += 1;
        }
    }
    for op in ["min_confidence_up", "min_confidence_down", "add_verifier", "reorder_independent", "remove_redundant"] {
        assert!(by_op.get(op).copied().unwrap_or(0) > 0, "{op} never applied: {by_op:?}");
    }
    let general = shipped_graph(tt("GENERAL_TASK"));
    let rm: Vec<_> = mutate::remove_redundant(&general).into_iter().map(|c| c.target.unwrap()).collect();
    assert_eq!(rm, vec!["G02"], "only G02's output is read by nobody");
    let up = mutate::min_confidence(&general, true);
    assert_eq!(up[0].graph["nodes"][5]["extensions"]["x-decision"]["min_confidence"], 0.8);
    let research = shipped_graph(tt("RESEARCH_DOC"));
    let av = mutate::add_verifier(&research);
    assert_eq!(av.iter().map(|c| c.target.clone().unwrap()).collect::<Vec<_>>(), vec!["R01"], "R04 already feeds a verifier");
    let d = mutate::diff(&research, &av[0].graph);
    assert!(d["summary"].as_array().unwrap().iter().any(|l| l == "+ edge R01_check -> R02 (NORMAL)"), "{d}");

    // S2 verification -> S1 decision with an S2 twin; it runs, and abstention escalates to the twin.
    let t = tt("GENERAL_TASK");
    let s2 = general_with_s2_review();
    mutate::validate(t, &s2, &s2).unwrap();
    let cands = mutate::s2_to_s1(&s2);
    assert_eq!(cands.len(), 1);
    let c = &cands[0].graph;
    mutate::validate(t, &s2, c).unwrap();
    let g05 = c["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == "G05").unwrap();
    assert_eq!((g05["cognitive_role"].as_str(), g05["on_failure"]["target"].as_str()), (Some("S1"), Some("G05_s2")));
    assert_eq!(g05["extensions"]["x-decision"]["pass"], json!(["pass"]));
    let fx = |script: Value| evals::Fixture { task_type: t.id.into(), goal: "g".into(), params: json!({}), script };
    let ok = evals::scripted_trajectory_on(&fx(json!({ "wait": { "G06": "open" } })), Some(c));
    assert_eq!(ok.status, "completed");
    let esc = evals::scripted_trajectory_on(&fx(json!({ "wait": { "G06": "open" }, "decide": { "G05": "abstain" } })), Some(c));
    assert!(esc.steps.iter().any(|s| s.node == "G05_s2" && s.ok) && esc.status == "completed", "{esc:?}");
    // Not offered on a graph that already decides at S1.
    assert!(mutate::s2_to_s1(&general).is_empty());

    // Guardrails: a writer losing its policy guard, a touched gate.
    let thread = tt("THREAD_WORK");
    let base = shipped_graph(thread);
    let mut bad = base.clone();
    bad["edges"] = json!(base["edges"].as_array().unwrap().iter().map(|e| {
        let mut e = e.clone();
        if e["from"] == "T04" && e["to"] == "T05" { e["to"] = json!("T06"); }
        e
    }).collect::<Vec<_>>());
    assert!(mutate::guardrails(thread, &base, &bad).is_err());
}

struct Stub(String);

#[async_trait]
impl Proposer for Stub {
    async fn complete(&self, _prompt: &str) -> Result<(String, u64, f64), String> {
        Ok((self.0.clone(), 321, 0.004))
    }
}

#[tokio::test]
async fn harness_llm_patches_are_validated_and_invalid_ones_discarded() {
    let t = tt("GENERAL_TASK");
    let base = shipped_graph(t);
    let gate = base["nodes"].as_array().unwrap().iter().position(|n| n["node_id"] == "gate").unwrap();
    let reply = json!({ "candidates": [
        { "rationale": "tighten review", "patch": [{ "op": "add", "path": "/nodes/5/extensions/x-decision/min_confidence", "value": 0.85 }] },
        { "rationale": "drop the gate", "patch": [{ "op": "remove", "path": format!("/nodes/{gate}") }] },
        { "rationale": "bad pointer", "patch": [{ "op": "replace", "path": "/nodes/99/node_id", "value": "x" }] },
        { "rationale": "give G03 write authority", "patch": [{ "op": "replace", "path": "/nodes/3/write_set", "value": ["fs:x"] }] },
        { "rationale": "not a graph", "patch": [{ "op": "replace", "path": "/nodes", "value": "x" }] },
        { "rationale": "unknown primitive", "patch": [{ "op": "replace", "path": "/nodes/2/primitive_id", "value": "nope.made_up" }] }
    ] });
    let (ok, bad, rep) = llm_candidates(&Stub(format!("Here you go:\n{reply}\n")), t, &base, &json!({}), 8).await;
    assert_eq!(ok.len(), 1, "{bad:?}");
    assert_eq!((ok[0].operator.as_str(), ok[0].graph["nodes"][5]["extensions"]["x-decision"]["min_confidence"].as_f64()), ("llm_patch", Some(0.85)));
    assert_eq!(bad.len(), 5);
    assert!(bad.iter().all(|b| b["operator"] == "llm_patch" && !b["reason"].as_str().unwrap_or_default().is_empty()), "{bad:?}");
    assert!(bad[1]["reason"].as_str().unwrap().contains("op 0"), "{}", bad[1]);
    assert_eq!((rep["tokens"].as_u64(), rep["candidates"].as_u64(), rep["returned"].as_u64()), (Some(321), Some(1), Some(6)));
    // k bounds what is read from the reply.
    let (ok, bad, _) = llm_candidates(&Stub(reply.to_string()), t, &base, &json!({}), 1).await;
    assert_eq!((ok.len(), bad.len()), (1, 0));
    // Garbage and errors produce no candidates.
    let (ok, bad, _) = llm_candidates(&Stub("I would remove the gate.".into()), t, &base, &json!({}), 4).await;
    assert_eq!((ok.len(), bad.len()), (0, 1));
    let (ok, _, rep) = llm_candidates(&NoProposer, t, &base, &json!({}), 4).await;
    assert!(ok.is_empty() && rep["candidates"] == 0);
    // RFC 6902 ops.
    let d = json!({ "a": [1, 2], "b": { "c": 1 } });
    let p = json!([{ "op": "test", "path": "/b/c", "value": 1 }, { "op": "add", "path": "/a/-", "value": 3 }, { "op": "move", "from": "/b/c", "path": "/d" },
                   { "op": "copy", "from": "/a/0", "path": "/a/0" }, { "op": "remove", "path": "/b" }]);
    assert_eq!(mutate::apply_patch(&d, &p).unwrap(), json!({ "a": [1, 1, 2, 3], "d": 1 }));
    assert!(mutate::apply_patch(&d, &json!([{ "op": "test", "path": "/b/c", "value": 2 }])).is_err());
    assert!(mutate::apply_patch(&d, &json!([])).is_err());
}

fn arm(outcomes: &[(&str, bool)], high: &[&str], cost: f64, latency: f64) -> Arm {
    Arm {
        outcomes: outcomes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        high_stakes: high.iter().map(|s| s.to_string()).collect(),
        cost_units: cost,
        latency_units: latency,
        ..Default::default()
    }
}

fn n_cases(n: usize, fail: &[usize]) -> Vec<(String, bool)> {
    (0..n).map(|i| (format!("c{i}"), !fail.contains(&i))).collect()
}

fn arm_n(n: usize, fail: &[usize], cost: f64, latency: f64) -> Arm {
    let o = n_cases(n, fail);
    arm(&o.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(), &[], cost, latency)
}

fn passes(r: &Rule, b: &Arm, c: &Arm) -> (bool, Value) {
    let v = decide(r, b, c, 7, 2000);
    (v["passed"] == true, v)
}

#[test]
fn harness_acceptance_rule_edge_cases() {
    let r = Rule::default();
    // Exactly +5 points passes; just under does not.
    let (ok, v) = passes(&r, &arm_n(20, &[0, 1, 2, 3, 4], 100.0, 100.0), &arm_n(20, &[1, 2, 3, 4], 100.0, 100.0));
    assert!(ok, "{v}");
    assert_eq!(v["delta"], 0.05);
    let (ok, v) = passes(&r, &arm_n(21, &[0, 1, 2, 3, 4], 100.0, 100.0), &arm_n(21, &[1, 2, 3, 4], 100.0, 100.0));
    assert!(!ok && v["reasons"][0].as_str().unwrap().starts_with("not enough gain"), "{v}");
    // Same pass rate: 10% cheaper passes, 9% does not; latency counts too.
    assert!(passes(&r, &arm_n(10, &[], 100.0, 100.0), &arm_n(10, &[], 90.0, 100.0)).0);
    assert!(!passes(&r, &arm_n(10, &[], 100.0, 100.0), &arm_n(10, &[], 91.0, 100.0)).0);
    assert!(passes(&r, &arm_n(10, &[], 100.0, 100.0), &arm_n(10, &[], 100.0, 80.0)).0);
    // Cheaper but worse beyond epsilon.
    let (ok, v) = passes(&r, &arm_n(10, &[], 100.0, 100.0), &arm_n(10, &[3], 10.0, 10.0));
    assert!(!ok && v["reasons"][0].as_str().unwrap().starts_with("pass rate regressed"), "{v}");
    // Within epsilon on the point estimate (-1 point), faster, but the interval reaches below -epsilon.
    let (ok, v) = passes(&r, &arm_n(100, &[], 100.0, 100.0), &arm_n(100, &[7], 100.0, 50.0));
    assert!(!ok, "{v}");
    assert!(v["reasons"].as_array().unwrap().iter().any(|x| x.as_str().unwrap().starts_with("delta interval lower bound")), "{v}");
    // High stakes: a new failure blocks even a big gain; an old one only under `none`.
    let b = arm(&[("a", true), ("b", false), ("c", false), ("h", true)], &["h"], 1.0, 1.0);
    let c = arm(&[("a", true), ("b", true), ("c", true), ("h", false)], &["h"], 1.0, 1.0);
    let (ok, v) = passes(&r, &b, &c);
    assert!(!ok && v["new_high_stakes_failures"] == json!(["h"]), "{v}");
    let b = arm(&[("a", true), ("b", false), ("c", false), ("d", false), ("e", false), ("h", false)], &["h"], 1.0, 1.0);
    let c = arm(&[("a", true), ("b", true), ("c", true), ("d", true), ("e", true), ("h", false)], &["h"], 1.0, 1.0);
    assert!(passes(&r, &b, &c).0, "a pre-existing high-stakes failure is not new");
    let strict = Rule { high_stakes: HighStakes::None, ..Rule::default() };
    assert!(!passes(&strict, &b, &c).0);
    // Nothing graded on both sides.
    let (ok, v) = passes(&r, &arm(&[("a", true)], &[], 1.0, 1.0), &arm(&[("b", true)], &[], 1.0, 1.0));
    assert!(!ok && v["paired_cases"] == 0);
    // Configurable rule, validated.
    let r0 = Rule::parse(Some(&json!({ "min_gain": 0, "epsilon": 0.05, "high_stakes": "none" }))).unwrap();
    assert_eq!((r0.min_gain, r0.epsilon, r0.high_stakes), (0.0, 0.05, HighStakes::None));
    assert!(passes(&r0, &arm_n(10, &[], 1.0, 1.0), &arm_n(10, &[], 1.0, 1.0)).0, "min_gain 0: a neutral candidate passes");
    for bad in [json!({ "min_gain": 0.9 }), json!({ "epsilon": -1 }), json!({ "high_stakes": "maybe" }), json!({ "x": 1 }), json!(3)] {
        assert!(Rule::parse(Some(&bad)).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn harness_offline_evaluation_scores_candidates_on_shipped_evals() {
    let t = tt("GENERAL_TASK");
    let base = shipped_graph(t);
    let cases = cases_for(t, &[], &[]);
    assert_eq!(cases.len(), 7);
    let b = evaluate(&Env::Offline, t, &base, &cases).await.unwrap();
    assert_eq!(b.pass_rate(), Some(1.0), "{}", b.summary());
    let rm = mutate::remove_redundant(&base).remove(0);
    let c = evaluate(&Env::Offline, t, &rm.graph, &cases).await.unwrap();
    assert_eq!(c.pass_rate(), Some(1.0));
    assert!(c.latency_units < b.latency_units && c.cost_units == b.cost_units);
    let v = decide(&Rule::default(), &b, &c, 1, 500);
    assert_eq!(v["passed"], false, "a 1-unit step is under the 10% latency bar: {v}");
    // A candidate that breaks a shipped case regresses.
    let mut broken = base.clone();
    for e in broken["edges"].as_array_mut().unwrap() {
        if e["from"] == "G03" && e["edge_kind"] == "FAILURE" { e["to"] = json!("G04"); }
    }
    for n in broken["nodes"].as_array_mut().unwrap() {
        if n["node_id"] == "G03" { n["on_failure"] = json!({ "strategy": "ESCALATE", "target": "G04" }); }
    }
    broken["nodes"].as_array_mut().unwrap().retain(|n| n["node_id"] != "G03_deep");
    broken["edges"].as_array_mut().unwrap().retain(|e| e["from"] != "G03_deep");
    if mutate::validate(t, &base, &broken).is_ok() {
        let c = evaluate(&Env::Offline, t, &broken, &cases).await.unwrap();
        let v = decide(&Rule::default(), &b, &c, 1, 500);
        assert_eq!(v["passed"], false, "{v}");
        assert!(c.failed_at.contains_key("G04") || c.pass_rate() < b.pass_rate(), "{}", c.summary());
    }
    // S2 -> S1 on a graph whose review is S2: same pass rate, far cheaper -> accepted.
    let s2 = general_with_s2_review();
    let b = evaluate(&Env::Offline, t, &s2, &cases_for(t, &[], &[])).await.unwrap();
    let c = evaluate(&Env::Offline, t, &mutate::s2_to_s1(&s2).remove(0).graph, &cases_for(t, &[], &[])).await.unwrap();
    let v = decide(&Rule::default(), &b, &c, 1, 500);
    assert!(c.cost_units < b.cost_units * 0.9, "{} vs {}", c.cost_units, b.cost_units);
    // The S2 graph fails the shipped cases that need G05_s2 / an S1 rejection; the S1 version passes them.
    assert!(c.pass_rate() >= b.pass_rate());
    assert_eq!(v["passed"], true, "{v}");
}

// ── end to end ──────────────────────────────────────────────────────────────

const ADMIN: &str = "u_harness_admin";
const USER: &str = "u_harness_user";

async fn finished_run(t: &T, key: &str, goal: &str) -> (String, RunRecord) {
    let (st, _, b) = call(&t.app, post("/v1/agency", USER, Some(key), json!({ "goal": goal, "task_type": "GENERAL_TASK" }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let limits = run_limits(&[(guard::ORGS_ENV, &format!("user:{USER}")), (guard::ORG_RUNS_PER_HOUR_ENV, "1000")]);
    assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await);
    let mut rec = wait_settled(&s, &id).await;
    for _ in 0..3 {
        if rec.run["status"] != "needs_attention" {
            break;
        }
        let att = rec.attention.iter().find(|a| a["status"] == "open").cloned().unwrap();
        let (st, _, b) = call(&t.app, post(&format!("/v1/attention/{}/responses", att["id"].as_str().unwrap()), USER, None, json!({ "type": "approval" }))).await;
        assert_eq!(st, StatusCode::OK, "{b}");
        assert!(executor::admit_and_start(t.st.clone(), id.clone(), limits.clone()).await);
        rec = wait_settled(&s, &id).await;
    }
    assert_eq!(rec.run["status"], "completed", "{}", rec.run);
    (id, rec)
}

async fn req(t: &T, method: &str, uri: &str, user: &str, body: Option<Value>) -> (StatusCode, Value) {
    let r = match (method, body) {
        ("GET", _) => get_req(uri, user),
        (_, b) => post(uri, user, None, b.unwrap_or(json!({}))),
    };
    let (st, _, b) = call(&t.app, r).await;
    (st, serde_json::from_str(&b).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn harness_proposals_approve_pin_version_and_rollback() {
    let _env = E2E_ENV.lock().await;
    let runs = tempfile::tempdir().unwrap();
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    std::env::set_var(task_types::ENABLE_ENV, format!("BUG_FIX,{}", task_types::ids().join(",")));
    std::env::set_var("ALLTERNIT_DECISIONS_ADMINS", ADMIN);
    let t = setup().await;
    let (src, _) = finished_run(&t, "c6-e2e-general-0001", "Summarize the notes").await;

    // Auth: decisions admins only.
    let ask = json!({ "task_type": "GENERAL_TASK", "operators": ["remove_redundant"], "acceptance": { "min_gain": 0 } });
    assert_eq!(req(&t, "POST", "/v1/harness/proposals", USER, Some(ask.clone())).await.0, StatusCode::FORBIDDEN);
    assert_eq!(req(&t, "GET", "/v1/harness/proposals", USER, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(req(&t, "POST", "/v1/harness/versions/GENERAL_TASK/rollback", USER, None).await.0, StatusCode::FORBIDDEN);
    // Validation.
    let (st, e) = req(&t, "POST", "/v1/harness/proposals", ADMIN, Some(json!({ "task_type": "NOPE" }))).await;
    assert_eq!((st, e["error"]["param"].as_str()), (StatusCode::BAD_REQUEST, Some("task_type")));
    let (st, e) = req(&t, "POST", "/v1/harness/proposals", ADMIN, Some(json!({ "task_type": "GENERAL_TASK", "operators": ["rewrite_everything"] }))).await;
    assert_eq!((st, e["error"]["param"].as_str()), (StatusCode::BAD_REQUEST, Some("operators")));
    let (st, _) = req(&t, "POST", "/v1/harness/proposals", ADMIN, Some(json!({ "task_type": "GENERAL_TASK", "effects": "live" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Search A: drop G02 (min_gain 0: a neutral, leaner graph is acceptable here).
    let (st, a) = req(&t, "POST", "/v1/harness/proposals", ADMIN, Some(ask)).await;
    assert_eq!(st, StatusCode::CREATED, "{a}");
    assert_eq!(a["status"], "completed", "{a}");
    assert_eq!(a["replay_runs"], json!([src]), "the finished run is replayed");
    assert!(a["cases"].as_array().unwrap().iter().any(|c| *c == format!("runs/{src}")));
    assert_eq!(a["evidence"]["runs"]["considered"], 1);
    assert_eq!(a["candidates"].as_array().unwrap().len(), 1, "{a}");
    let pa = a["pending"][0].as_str().unwrap_or_else(|| panic!("not pending: {}", a["candidates"])).to_string();
    // Search B: reorder (two independent pairs) -> pending, evaluated against version 1 too.
    let (st, b) = req(&t, "POST", "/v1/harness/proposals", ADMIN,
        Some(json!({ "task_type": "GENERAL_TASK", "operators": ["reorder_independent"], "acceptance": { "min_gain": 0 }, "replay_runs": 0 }))).await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    let pb = b["pending"][0].as_str().unwrap_or_else(|| panic!("{b}")).to_string();
    // Default rule: the same G02 removal is not enough gain.
    let (_, c) = req(&t, "POST", "/v1/harness/proposals", ADMIN, Some(json!({ "task_type": "GENERAL_TASK", "operators": ["remove_redundant"], "replay_runs": 0 }))).await;
    assert_eq!((c["candidates"][0]["status"].as_str(), c["pending"].as_array().map(Vec::len)), (Some("failed_acceptance"), Some(0)), "{c}");
    assert!(c["candidates"][0]["reasons"][0].as_str().unwrap().starts_with("not enough gain"), "{c}");

    // The pending list and a proposal's diff and evidence.
    let (st, l) = req(&t, "GET", "/v1/harness/proposals?status=pending&task_type=general_task", ADMIN, None).await;
    assert_eq!(st, StatusCode::OK);
    let ids: Vec<&str> = l["data"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&pa.as_str()) && ids.contains(&pb.as_str()) && ids.len() >= 2, "{l}");
    let (_, p) = req(&t, "GET", &format!("/v1/harness/proposals/{pa}"), ADMIN, None).await;
    assert_eq!(p["diff"]["nodes_removed"], json!(["G02"]), "{}", p["diff"]);
    assert!(p["diff"]["summary"].as_array().unwrap().iter().any(|l| l == "+ edge G01 -> G03 (NORMAL)"));
    assert_eq!(p["acceptance"]["passed"], true);
    assert!(p["evidence"]["baseline_eval"].is_object() && p["metrics"]["candidate"]["graded"].as_u64() == Some(8), "{}", p["metrics"]);

    // Approve A -> version 2 is active.
    let (st, ap) = req(&t, "POST", &format!("/v1/harness/proposals/{pa}/approve"), ADMIN, Some(json!({ "note": "leaner" }))).await;
    assert_eq!(st, StatusCode::OK, "{ap}");
    assert_eq!((ap["versions"]["active_version"].as_u64(), ap["proposal"]["status"].as_str()), (Some(2), Some("approved")));
    assert_eq!(ap["versions"]["versions"][0]["graph_id"], "general.task.v2");
    assert!(ap["versions"]["versions"][0].get("graph").is_none(), "registry view omits graph bodies");
    let (st, e) = req(&t, "POST", &format!("/v1/harness/proposals/{pa}/approve"), ADMIN, None).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("ERR_HARNESS_NOT_PENDING")));
    // B was evaluated against version 1: stale now.
    let (st, e) = req(&t, "POST", &format!("/v1/harness/proposals/{pb}/approve"), ADMIN, None).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("ERR_HARNESS_STALE")));
    let (st, r) = req(&t, "POST", &format!("/v1/harness/proposals/{pb}/reject"), ADMIN, Some(json!({ "note": "stale" }))).await;
    assert_eq!((st, r["status"].as_str()), (StatusCode::OK, Some("rejected")));

    // New runs pin version 2 and run it (no G02 step).
    let (v2run, rec) = finished_run(&t, "c6-e2e-general-0002", "Summarize again").await;
    assert_eq!(rec.task_ir["graph_version"]["version"], 2);
    assert_eq!(rec.task_ir["graph_version"]["graph_id"], "general.task.v2");
    assert!(!rec.task_ir["nodes"].as_array().unwrap().iter().any(|n| n["id"] == "G02"));
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let steps: Vec<String> = s.events(&v2run).await.unwrap().iter().filter(|e| e["type"] == "run.progress")
        .filter_map(|e| e["data"]["step"].as_str().map(String::from)).collect();
    assert!(steps.iter().any(|x| x == "G03") && !steps.iter().any(|x| x == "G02"), "{steps:?}");
    // The earlier run keeps the shipped graph.
    let old = s.load_run(&src).await.unwrap().unwrap();
    assert!(old.task_ir.get("graph_version").is_none());

    // Rollback -> shipped graph for new runs; the proposal is marked rolled back.
    let (st, rb) = req(&t, "POST", "/v1/harness/versions/GENERAL_TASK/rollback", ADMIN, Some(json!({ "note": "back" }))).await;
    assert_eq!((st, rb["active_version"].as_u64()), (StatusCode::OK, Some(1)), "{rb}");
    assert!(rb["versions"][0]["rolled_back_at"].is_string());
    let (_, p) = req(&t, "GET", &format!("/v1/harness/proposals/{pa}"), ADMIN, None).await;
    assert_eq!(p["status"], "rolled_back");
    let (st, e) = req(&t, "POST", "/v1/harness/versions/GENERAL_TASK/rollback", ADMIN, None).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("ERR_HARNESS_NOTHING_TO_ROLL_BACK")));
    let (_, rec) = finished_run(&t, "c6-e2e-general-0003", "Summarize once more").await;
    assert!(rec.task_ir.get("graph_version").is_none());
    assert!(rec.task_ir["nodes"].as_array().unwrap().iter().any(|n| n["id"] == "G02"));

    // Every transition audited.
    let (_, au) = req(&t, "GET", "/v1/harness/audit?task_type=GENERAL_TASK", ADMIN, None).await;
    let actions: BTreeSet<&str> = au["data"].as_array().unwrap().iter().filter_map(|a| a["action"].as_str()).collect();
    for a in ["proposal.created", "proposal.approved", "proposal.rejected", "version.rolled_back"] {
        assert!(actions.contains(a), "{a} missing: {au}");
    }
    let (st, v) = req(&t, "GET", "/v1/harness/versions/general_task", ADMIN, None).await;
    assert_eq!((st, v["stack"].clone()), (StatusCode::OK, json!([1])));
}
