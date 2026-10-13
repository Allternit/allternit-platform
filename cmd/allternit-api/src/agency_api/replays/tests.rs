//! B4: replay overrides and batch replays. Unit tests for validation, the
//! cognition walk, decision re-deciding and aggregation; end to end through
//! the Agency API with the scripted executor (no model, no real effect).

use super::sim::{self, Cognition, Gen, Recording, SimConfig};
use super::*;
use crate::agency_api::decisions::{flywheel, store as dstore};
use crate::agency_api::tests::{call, get_req, post, run_limits, setup, wait_settled, T, E2E_ENV};
use crate::agency_api::{executor, guard, store::AgencyStore};
use crate::db::DbHandle;
use allternit_factory_engine::kernel::graph::GraphNode;
use async_trait::async_trait;

// ── overrides validation ────────────────────────────────────────────────────

fn ov(v: Value) -> Result<Overrides, Fail> {
    parse_overrides(Some(&v))
}

#[test]
fn replay_overrides_validate_and_normalize() {
    assert!(!ov(json!({})).unwrap().cognition());
    assert!(parse_overrides(None).unwrap().heads.is_empty());
    let o = ov(json!({ "decision_heads": { "element": "head.lookup", "verify": { "head": "head.lookup", "version": "1" } },
                       "model_class": "mc.local", "graph_version": "general.task.v1" })).unwrap();
    assert!(o.cognition());
    assert_eq!(o.head_for("element").unwrap().name(), "head.lookup");
    let e = o.echo();
    assert_eq!(e["decision_heads"]["verify"], json!({ "head": "head.lookup", "version": "1" }));
    assert_eq!(e["model_class"], "mc.local");
    assert_eq!(e["graph_version"], json!({ "graph_id": "general.task.v1", "task_type": "GENERAL_TASK", "inline": false }));
    assert!(ov(json!({ "model_class": "mc.remote.fast" })).is_ok());
    assert!(ov(json!({ "graph_version": { "graph_id": "research.doc.v1" } })).is_ok());

    let bad = |v: Value, param: &str, code: &str| {
        let f = ov(v.clone()).err().unwrap_or_else(|| panic!("accepted {v}"));
        assert_eq!((f.param.as_deref(), f.code), (Some(param), code), "{v}: {}", f.message);
    };
    bad(json!({ "completion": {} }), "overrides.completion", "ERR_INPUT_UNSUPPORTED");
    bad(json!({ "decision_heads": {} }), "overrides.decision_heads", "ERR_INPUT_INVALID");
    bad(json!({ "decision_heads": { "element": "head.nope" } }), "overrides.decision_heads.element", "ERR_INPUT_INVALID");
    bad(json!({ "decision_heads": { "element": { "head": "head.lookup", "version": "9" } } }), "overrides.decision_heads.element", "ERR_INPUT_INVALID");
    bad(json!({ "decision_heads": { "element": { "head": "head.lookup", "x": 1 } } }), "overrides.decision_heads.element", "ERR_INPUT_UNSUPPORTED");
    bad(json!({ "decision_heads": { "bad kind!": "head.lookup" } }), "overrides.decision_heads.bad kind!", "ERR_INPUT_INVALID");
    bad(json!({ "model_class": "gpu-big" }), "overrides.model_class", "ERR_INPUT_INVALID");
    bad(json!({ "model_class": 3 }), "overrides.model_class", "ERR_INPUT_INVALID");
    bad(json!({ "graph_version": "nope.v9" }), "overrides.graph_version", "ERR_INPUT_INVALID");
    bad(json!({ "graph_version": { "graph": { "nodes": "x" } } }), "overrides.graph_version", "ERR_INPUT_INVALID");
    bad(json!({ "graph_version": { "graph_id": "a", "graph": serde_json::from_str::<Value>(general_json()).unwrap() } }), "overrides.graph_version", "ERR_INPUT_INVALID");
    assert_eq!(ov(json!([1])).err().unwrap().param.as_deref(), Some("overrides"));
}

fn general_json() -> &'static str {
    include_str!("../task_types/graphs/general.task.v1.json")
}

#[test]
fn replay_recorded_only_is_asserted_and_fails_closed() {
    assert!(assert_recorded_only(Some(0), 0).is_ok());
    for (k, w) in [(Some(1), 0), (None, 0), (Some(0), 1)] {
        let f = assert_recorded_only(k, w).err().unwrap();
        assert_eq!((f.status, f.code), (500, "ERR_REPLAY_LIVE_EFFECT"));
    }
    // Only recorded_only is a mode.
    let m = json!({ "effects": "live" });
    assert_eq!(check_effects(m.as_object().unwrap()).err().unwrap().param.as_deref(), Some("effects"));
    assert!(check_effects(json!({ "effects": "recorded_only" }).as_object().unwrap()).is_ok());
}

#[test]
fn replay_batch_selection_validates_and_picks_recent_finished_runs() {
    let sel = |v: Value| parse_selection(v.as_object().unwrap());
    assert_eq!(sel(json!({ "runs": ["run_a", "run_b"] })).unwrap(), Selection::Runs(vec!["run_a".into(), "run_b".into()]));
    assert!(sel(json!({ "runs": [] })).is_err());
    assert!(sel(json!({ "runs": ["run_a", "run_a"] })).is_err());
    assert!(sel(json!({ "runs": ["a"], "selector": {} })).is_err());
    assert!(sel(json!({})).is_err());
    assert_eq!(sel(json!({ "selector": { "limit": 0 } })).err().unwrap().param.as_deref(), Some("selector.limit"));
    assert_eq!(sel(json!({ "selector": { "since": "yesterday" } })).err().unwrap().param.as_deref(), Some("selector.since"));
    assert_eq!(sel(json!({ "selector": { "task_type": "NOPE" } })).err().unwrap().param.as_deref(), Some("selector.task_type"));
    assert_eq!(sel(json!({ "selector": { "nope": 1 } })).err().unwrap().code, "ERR_INPUT_UNSUPPORTED");
    let Selection::Recent { task_type, since, limit } = sel(json!({ "selector": { "task_type": "general_task", "since": "2026-10-01T00:00:00Z" } })).unwrap() else { panic!() };
    assert_eq!((task_type.as_deref(), limit), (Some("GENERAL_TASK"), 20));
    let run = |id: &str, status: &str, tt: &str, at: &str| RunRecord {
        owner: "u".into(), idempotency_key: None, attention: vec![],
        run: json!({ "id": id, "status": status, "created_at": at }), task_ir: json!({ "task_type": tt }),
    };
    let picked = select_recent(vec![
        run("r1", "completed", "GENERAL_TASK", "2026-10-02T00:00:00Z"),
        run("r2", "running", "GENERAL_TASK", "2026-10-05T00:00:00Z"),
        run("r3", "failed", "GENERAL_TASK", "2026-10-04T00:00:00Z"),
        run("r4", "cancelled", "GENERAL_TASK", "2026-10-04T00:00:00Z"),
        run("r5", "completed", "RESEARCH_DOC", "2026-10-04T00:00:00Z"),
        run("r6", "completed", "GENERAL_TASK", "2026-09-01T00:00:00Z"),
        run("r7", "partial", "GENERAL_TASK", "2026-10-03T00:00:00Z"),
    ], task_type.as_deref(), since, 2);
    assert_eq!(picked.iter().map(|r| r.run["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["r3", "r7"], "newest finished first, limited");
}

#[test]
fn replay_batch_run_metrics_aggregate() {
    let s = |verdict, src, rep, live_tokens| RunSummary {
        verdict, simulated: true, source_pass: Some(src), replay_pass: Some(rep), source_tokens: 100, source_cost_usd: 0.01,
        source_latency_ms: 1000.0, live: sim::Spend { calls: 2, tokens: live_tokens, cost_usd: 0.004, latency_ms: 300.0 },
        source_regenerated: sim::Spend { calls: 2, tokens: 60, cost_usd: 0.006, latency_ms: 600.0 },
    };
    let m = batch_run_metrics(&[s("equivalent", true, true, 50), s("diverged_expected", true, false, 70), s("diverged_unexpected", false, false, 10)], 4);
    assert_eq!((m["requested"].as_u64(), m["replayed"].as_u64(), m["skipped"].as_u64()), (Some(4), Some(3), Some(1)));
    assert_eq!(m["verdicts"], json!({ "equivalent": 1, "diverged_expected": 1, "diverged_unexpected": 1 }));
    assert!((m["verifier"]["source_pass_rate"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert!((m["verifier"]["pass_at_1"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
    assert!((m["verifier"]["agreement"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!((m["tokens"]["live"].as_u64(), m["tokens"]["source_regenerated"].as_u64(), m["live_model_calls"].as_u64()), (Some(130), Some(180), Some(6)));
}

// ── decision-head override ──────────────────────────────────────────────────

const OWNER: &str = "u_rpl";

/// One stored decision of `run` (as `finish` stores it), optionally with an outcome.
fn decision(db: &DbHandle, id: &str, run: Option<&str>, ctx: &str, served: &str, outcome: Option<(&str, Option<&str>)>, latency: f64) {
    let ids = ["a", "b", "c"];
    let key = flywheel::key_hash("element", ctx, None, &ids, &["A", "B", "C"]);
    let opts = json!([{ "id": "a", "text": "A" }, { "id": "b", "text": "B" }, { "id": "c", "text": "C" }]);
    let attempts = json!([{ "backend": "oracle", "detail": { "cost_usd": 0.002 } }]);
    dstore::insert(db, &dstore::Row {
        id, owner: OWNER, created_at: &now(), kind: "element", session_id: run, task: None, context_hash: ctx, has_image: false,
        options: &opts, choice: Some(served), probs: &json!({}), confidence: 0.9, abstained: false, backend: "oracle",
        latency_ms: latency, attempts: &attempts,
    }).unwrap();
    let s = flywheel::Shadow { decision_id: id.into(), owner: OWNER.into(), kind: "element".into(), key, ids: ids.iter().map(|x| x.to_string()).collect(), has_image: false };
    flywheel::record_key(db, &s).unwrap();
    if let Some((status, label)) = outcome {
        dstore::set_outcome(db, id, OWNER, status, label, None, &now()).unwrap();
        flywheel::on_outcome(db, id, OWNER).unwrap();
    }
}

fn seed_decisions(db: &DbHandle, run: &str) {
    // History outside the run: "b" is right for ctxK, three times.
    for i in 0..3 {
        decision(db, &format!("h{i}"), None, "ctxK", "b", Some(("success", None)), 900.0);
    }
    // The run: served "a" on ctxK, which was wrong ("b" was right).
    decision(db, &format!("{run}_d1"), Some(run), "ctxK", "a", Some(("failure", Some("b"))), 800.0);
    // A context the head never saw: served "c", right.
    decision(db, &format!("{run}_d2"), Some(run), "ctxNew", "c", Some(("success", None)), 700.0);
    // Only its own outcome taught the head ctxSelf: leave-one-out leaves it nothing.
    decision(db, &format!("{run}_d3"), Some(run), "ctxSelf", "a", Some(("success", None)), 600.0);
    // No outcome: not labelled.
    decision(db, &format!("{run}_d4"), Some(run), "ctxK", "b", None, 500.0);
}

#[test]
fn replay_decision_head_override_redecides_leave_one_out_and_compares() {
    let db = DbHandle::new_memory().unwrap();
    seed_decisions(&db, "run_x");
    let conn = db.connect().unwrap();
    let recorded = decide::load(&conn, OWNER, "run_x").unwrap();
    assert_eq!(recorded.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["run_x_d1", "run_x_d2", "run_x_d3", "run_x_d4"]);
    assert!((recorded[0].cost_usd - 0.002).abs() < 1e-12);
    let head = flywheel::head_by_name("head.lookup").unwrap();
    let lookup_before: i64 = conn.query_row("SELECT SUM(successes) + SUM(failures) FROM decision_lookup", [], |r| r.get(0)).unwrap();
    let mut rows = vec![];
    let mut out = vec![];
    for d in &recorded {
        let r = decide::redecide(&conn, OWNER, d, head, 5).unwrap();
        assert_eq!(r.samples.len(), 1, "deterministic head: one sample");
        let (v, row) = decide::compare(d, head, &r);
        out.push(v);
        rows.push(row);
    }
    // d1: the head (3 successes for b, its own +1 taken out) answers b, confidently; right where the served a was wrong.
    assert_eq!(out[0]["candidate"]["choice"], "b");
    assert_eq!((out[0]["effective_choice"].as_str(), out[0]["correct"].as_bool(), out[0]["served_correct"].as_bool()), (Some("b"), Some(true), Some(false)));
    assert_eq!(out[0]["agrees_with_served"], false);
    // d2: no answer; the chain serves what it served.
    assert_eq!((out[1]["candidate"]["answered"].as_bool(), out[1]["effective_choice"].as_str()), (Some(false), Some("c")));
    // d3: leave-one-out: without its own outcome the head knows nothing.
    assert_eq!(out[2]["candidate"]["answered"], false, "{}", out[2]);
    assert_eq!(out[3]["label"], Value::Null);
    // Nothing was written: the lookup table is as it was.
    let lookup_after: i64 = conn.query_row("SELECT SUM(successes) + SUM(failures) FROM decision_lookup", [], |r| r.get(0)).unwrap();
    assert_eq!(lookup_before, lookup_after);
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM decision_shadow", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0, "a replay records no shadow rows");

    let m = decide::metrics_by_kind(&rows, &|_| 5);
    assert_eq!((m["replayed"].as_u64(), m["labelled"].as_u64(), m["answered"].as_u64()), (Some(4), Some(3), Some(2)));
    assert_eq!(m["accuracy"].as_f64(), Some(1.0));
    assert!((m["baseline_accuracy"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(m["head_accuracy"].as_f64(), Some(1.0));
    assert!((m["agreement_with_served"].as_f64().unwrap() - 0.75).abs() < 1e-9, "d4: head says b, b was served");
    assert!(m["latency_ms"].as_f64().unwrap() < m["baseline_latency_ms"].as_f64().unwrap());
    assert!(m["cost_usd"].as_f64().unwrap() < m["baseline_cost_usd"].as_f64().unwrap());
    assert_eq!((m["pass_at_1"].as_f64(), m["pass_at_k"].as_f64(), m["deterministic"].as_bool()), (Some(1.0), Some(1.0), Some(true)));
    assert_eq!(m["by_kind"]["element"]["head"], "head.lookup");
}

// ── the cognition walk ──────────────────────────────────────────────────────

struct Fixed(&'static str);

#[async_trait]
impl Cognition for Fixed {
    async fn generate(&self, _node: &GraphNode, _prompt: &str) -> Result<Gen, String> {
        Ok(Gen { text: self.0.into(), tokens: 7, cost_usd: 0.001, latency_ms: 3.0 })
    }
}

fn research() -> (allternit_factory_engine::kernel::graph::ComputeGraph, Vec<String>) {
    let ws = vec!["fs:docs/brief.md".to_string()];
    let t = task_types::get("RESEARCH_DOC").unwrap();
    (task_types::instantiate(t, "task.x", &ws).unwrap(), ws)
}

/// A recording of a completed RESEARCH_DOC run with `text` as every model output.
fn research_recording(text: &str, with_effect: bool) -> Recording {
    let (g, ws) = research();
    let mut r = Recording { graph: Some(g.clone()), tokens: 40, cost_usd: 0.004, duration_ms: 1000, ..Default::default() };
    // R09 (the review) is an S1 decision (B1): the scripted rule answers it, no generation.
    for n in ["R01", "R04"] {
        r.outputs.insert(n.into(), text.into());
    }
    // R07 writes its input (the document; the POLICY node adds no output), as the driver does.
    let content = crate::agency_api::executor::generic::effect_content(&Default::default(), text);
    let args = crate::agency_api::executor::generic::effect_args("R07", &ws, &content);
    if with_effect {
        r.effects.insert("R07".into(), sim::RecordedEffect { status: "committed".into(), result: Some("effect:fs:docs/brief.md:sha".into()),
            args_hash: allternit_factory_engine::receipts::jcs::hash_value(&args).unwrap() });
    }
    for n in ["R00", "R01", "R02", "R03", "R04", "R05", "R06", "R07", "R08", "R09", "gate", "persist"] {
        r.path.push(n.into());
        r.progress.insert(n.into(), sim::NodeRun { outcome: "committed".into(), tokens: if ["R01", "R04"].contains(&n) { 10 } else { 0 }, duration_ms: 100 });
    }
    r
}

async fn walk(rec: &Recording, cog: &dyn Cognition) -> sim::SimOut {
    let (g, ws) = research();
    let policy = task_types::completion_policy(task_types::get("RESEARCH_DOC").unwrap()).unwrap();
    let cfg = SimConfig { graph: &g, policy: &policy, goal: "brief", write_set: &ws, regen_all: true, tainted_from_start: false, scripted: true,
        regenerate: true, heads: &[], db: None, owner: OWNER, run_id: "run_r" };
    sim::simulate(&cfg, rec, cog).await
}

// ── S1 decision nodes (B1) ──────────────────────────────────────────────────

/// A stored `verify` decision over G05's options (pass, fail) on one context.
fn verify_decision(db: &DbHandle, id: &str, run: Option<&str>, served: &str, outcome: &str) {
    let ids = ["pass", "fail"];
    let key = flywheel::key_hash("verify", "ctxV", None, &ids, &["P", "F"]);
    let opts = json!([{ "id": "pass", "text": "P" }, { "id": "fail", "text": "F" }]);
    dstore::insert(db, &dstore::Row {
        id, owner: OWNER, created_at: &now(), kind: "verify", session_id: run, task: run.map(|r| format!("{r}:G05")).as_deref(), context_hash: "ctxV",
        has_image: false, options: &opts, choice: Some(served), probs: &json!({}), confidence: 0.9, abstained: false, backend: "oracle",
        latency_ms: 400.0, attempts: &json!([]),
    }).unwrap();
    let s = flywheel::Shadow { decision_id: id.into(), owner: OWNER.into(), kind: "verify".into(), key, ids: ids.iter().map(|x| x.to_string()).collect(), has_image: false };
    flywheel::record_key(db, &s).unwrap();
    dstore::set_outcome(db, id, OWNER, outcome, None, None, &now()).unwrap();
    flywheel::on_outcome(db, id, OWNER).unwrap();
}

/// A completed GENERAL_TASK recording whose G05 (S1) served `pass` as decision `dec_g05`.
fn general_recording() -> (allternit_factory_engine::kernel::graph::ComputeGraph, Recording) {
    let g = task_types::instantiate(task_types::get("GENERAL_TASK").unwrap(), "task.x", &[]).unwrap();
    let mut r = Recording { graph: Some(g.clone()), ..Default::default() };
    r.outputs.insert("G03".into(), "the response".into());
    r.decisions.insert("G05".into(), crate::agency_api::task_types::s1::Answer {
        decision_id: Some("dec_g05".into()), choice: Some("pass".into()), confidence: 0.9, abstained: false, backend: "oracle".into() });
    r.waits.insert("G06".into(), true);
    for n in ["G00", "G01", "G02", "G03", "G04", "G05", "G06", "gate", "persist"] {
        r.path.push(n.into());
        r.progress.insert(n.into(), sim::NodeRun { outcome: "committed".into(), tokens: 0, duration_ms: 10 });
    }
    (g, r)
}

async fn walk_general(rec: &Recording, g: &allternit_factory_engine::kernel::graph::ComputeGraph, heads: &[HeadOverride], db: &DbHandle) -> sim::SimOut {
    let policy = task_types::completion_policy(task_types::get("GENERAL_TASK").unwrap()).unwrap();
    let cfg = SimConfig { graph: g, policy: &policy, goal: "g", write_set: &[], regen_all: false, tainted_from_start: false, scripted: false,
        regenerate: false, heads, db: Some(db), owner: OWNER, run_id: "run_g" };
    sim::simulate(&cfg, rec, &Fixed("unused")).await
}

#[tokio::test]
async fn replay_walk_serves_the_journaled_s1_decision() {
    let db = DbHandle::new_memory().unwrap();
    let (g, rec) = general_recording();
    let o = walk_general(&rec, &g, &[], &db).await;
    assert_eq!((o.status, o.verifier), ("completed", Some(true)), "{}", o.reason);
    assert!(o.divergences.is_empty(), "{:?}", o.divergences);
    let g05 = o.nodes.iter().find(|n| n["node_id"] == "G05").unwrap();
    assert_eq!((g05["via"].as_str(), g05["decision"]["choice"].as_str()), (Some("recorded"), Some("pass")), "{g05}");
    assert_eq!(o.live.calls, 0, "nothing re-generated, nothing decided live");
    // Without a recorded decision a live (non-scripted) replay refuses the node: it never decides.
    let mut bare = general_recording().1;
    bare.decisions.clear();
    let o = walk_general(&bare, &g, &[], &db).await;
    assert_eq!(o.status, "stopped");
    assert!(o.divergences.iter().any(|d| d["type"] == "UNRECORDED" && d["step_ref"] == "G05"), "{:?}", o.divergences);
}

#[tokio::test]
async fn replay_head_override_changes_the_s1_choice_and_the_route() {
    let db = DbHandle::new_memory().unwrap();
    // History: "fail" was right five times on this context; the run served "pass" and the run failed.
    for i in 0..5 {
        verify_decision(&db, &format!("hv{i}"), None, "fail", "success");
    }
    verify_decision(&db, "dec_g05", Some("run_g"), "pass", "failure");
    let ids = sim::journaled_decision_ids(&db, "run_g").unwrap();
    assert!(ids.is_empty(), "no journal rows in this unit db");
    let (g, rec) = general_recording();
    let heads = vec![HeadOverride { kind: "verify".into(), head: flywheel::head_by_name("head.lookup").unwrap() }];
    let o = walk_general(&rec, &g, &heads, &db).await;
    // The head says "fail" at G05: a confident rejection, the run fails there; G06 onwards never run.
    assert_eq!(o.status, "failed", "{}", o.reason);
    assert_eq!(o.reason, "G05 decided `fail`");
    assert!(o.decisions_covered.contains("dec_g05"));
    let g05 = o.nodes.iter().find(|n| n["node_id"] == "G05").unwrap();
    assert_eq!((g05["via"].as_str(), g05["decision"]["choice"].as_str(), g05["outcome"].as_str()), (Some("candidate_head"), Some("fail"), Some("failed")), "{g05}");
    let dec = o.divergences.iter().find(|d| d["type"] == "DECISION").expect("a DECISION divergence");
    assert_eq!((dec["step_ref"].as_str(), dec["kind"].as_str()), (Some("G05"), Some("expected")));
    assert!(o.divergences.iter().any(|d| d["type"] == "MISSING_NODE" && d["step_ref"] == "G06"));
    assert!(o.divergences.iter().all(|d| d["kind"] == "expected"), "{:?}", o.divergences);
    // A head for another kind leaves the S1 node on its recording.
    let other = vec![HeadOverride { kind: "element".into(), head: flywheel::head_by_name("head.lookup").unwrap() }];
    let o = walk_general(&rec, &g, &other, &db).await;
    assert_eq!((o.status, o.divergences.len()), ("completed", 0), "{:?}", o.divergences);
    // Nothing was learned from the replay.
    let n: i64 = db.connect().unwrap().query_row("SELECT COUNT(*) FROM decision_shadow", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn replay_walk_same_output_is_equivalent_and_serves_recorded_effects() {
    let rec = research_recording("same text", true);
    let o = walk(&rec, &Fixed("same text")).await;
    assert_eq!((o.status, o.verifier), ("completed", Some(true)), "{}", o.reason);
    assert!(o.divergences.is_empty(), "{:?}", o.divergences);
    assert_eq!((o.live_effects, o.effects_served, o.live.calls, o.live.tokens), (0, 1, 2, 14));
    assert_eq!((o.source_regenerated.tokens, o.source_regenerated.calls), (20, 2));
    let r07 = o.nodes.iter().find(|n| n["node_id"] == "R07").unwrap();
    assert_eq!((r07["via"].as_str(), r07["effect"]["request_matches"].as_bool()), (Some("recorded"), Some(true)));
    assert!(o.nodes.iter().filter(|n| n["cognitive_role"] == "S2").all(|n| n["via"] == "live" && n["output_matches"] == true));
}

#[tokio::test]
async fn replay_walk_new_output_is_explained_and_effects_are_never_executed() {
    let rec = research_recording("recorded text", true);
    let o = walk(&rec, &Fixed("candidate text")).await;
    assert_eq!(o.status, "completed", "{}", o.reason);
    assert_eq!(o.live_effects, 0);
    let types: Vec<&str> = o.divergences.iter().map(|d| d["type"].as_str().unwrap()).collect();
    assert!(types.contains(&"OUTPUT") && types.contains(&"EFFECT_REQUEST"), "{types:?}");
    assert!(o.divergences.iter().all(|d| d["kind"] == "expected"), "{:?}", o.divergences);
    let r07 = o.nodes.iter().find(|n| n["node_id"] == "R07").unwrap();
    assert_eq!((r07["effect"]["served"].as_str(), r07["effect"]["request_matches"].as_bool()), (Some("recorded"), Some(false)));
    // No recorded effect: refused, the walk stops, nothing runs.
    let o = walk(&research_recording("recorded text", false), &Fixed("recorded text")).await;
    assert_eq!((o.status, o.effects_refused, o.live_effects), ("stopped", 1, 0));
    let last = o.divergences.iter().find(|d| d["type"] == "UNRECORDED").unwrap();
    assert_eq!((last["step_ref"].as_str(), last["kind"].as_str()), (Some("R07"), Some("unexpected")));
    assert!(o.divergences.iter().any(|d| d["type"] == "MISSING_NODE" && d["step_ref"] == "R08"));
}

// ── end to end ──────────────────────────────────────────────────────────────

const USER: &str = "u_rpl_admin";

async fn finished_run(t: &T, key: &str, tt: &str, goal: &str, resources: Option<Value>) -> String {
    let mut body = json!({ "goal": goal, "task_type": tt });
    if let Some(r) = resources {
        body["workspace"] = json!({ "resources": r });
    }
    let (st, _, b) = call(&t.app, post("/v1/agency", USER, Some(key), body)).await;
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
    id
}

async fn replay(t: &T, body: Value) -> (StatusCode, Value) {
    let (st, _, b) = call(&t.app, post("/v1/replays", USER, None, body)).await;
    (st, serde_json::from_str(&b).unwrap_or(Value::Null))
}

async fn results(t: &T, id: &str) -> Value {
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/replays/{id}/results"), USER)).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    serde_json::from_str(&b).unwrap()
}

fn journal_rows(t: &T, run: &str) -> i64 {
    t.st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM agency_effect_journal WHERE run_id = ?1", [run], |r| r.get(0)).unwrap()
}

fn receipts(t: &T, run: &str) -> usize {
    t.st.rails.receipts.chain_store().unwrap().read_run(run).unwrap().len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_overrides_end_to_end_recorded_only() {
    let _env = E2E_ENV.lock().await;
    let runs = tempfile::tempdir().unwrap();
    std::env::set_var("ALLTERNIT_AGENCY_COGNITION", "scripted");
    std::env::set_var("ALLTERNIT_AGENCY_RUNS_DIR", runs.path());
    std::env::set_var(task_types::ENABLE_ENV, format!("BUG_FIX,{}", task_types::ids().join(",")));
    std::env::set_var("ALLTERNIT_DECISIONS_ADMINS", USER);
    let t = setup().await;
    let general = finished_run(&t, "b4-e2e-general-0001", "GENERAL_TASK", "Summarize the notes", None).await;
    let research = finished_run(&t, "b4-e2e-research-0001", "RESEARCH_DOC", "Write a brief", Some(json!(["fs:docs/brief.md"]))).await;
    let (j0, c0) = (journal_rows(&t, &research), receipts(&t, &research));

    // Validation through the API.
    let (st, e) = replay(&t, json!({ "source_run_id": general, "effects": "live" })).await;
    assert_eq!((st, e["error"]["param"].as_str()), (StatusCode::BAD_REQUEST, Some("effects")));
    let (st, e) = replay(&t, json!({ "source_run_id": general, "overrides": { "models": {} } })).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("ERR_INPUT_UNSUPPORTED")));
    // A graph that cannot take this run's write set.
    let (st, e) = replay(&t, json!({ "source_run_id": general, "overrides": { "graph_version": "research.doc.v1" } })).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("ERR_REPLAY_GRAPH_INVALID")), "{e}");

    // model_class on RESEARCH_DOC: cognition re-run, the fs effect answered from the recording.
    let (st, r) = replay(&t, json!({ "source_run_id": research, "effects": "recorded_only", "overrides": { "model_class": "mc.local" } })).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{r}");
    assert_eq!((r["status"].as_str(), r["effects"].as_str(), r["overrides"]["model_class"].as_str()), (Some("completed"), Some("recorded_only"), Some("mc.local")));
    let res = results(&t, r["id"].as_str().unwrap()).await;
    assert_eq!(res["verdict"], "equivalent", "{}", res["divergences"]);
    let nodes = res["nodes"].as_array().unwrap();
    let r07 = nodes.iter().find(|n| n["node_id"] == "R07").unwrap();
    assert_eq!((r07["via"].as_str(), r07["effect"]["request_matches"].as_bool()), (Some("recorded"), Some(true)), "{r07}");
    assert!(nodes.iter().any(|n| n["node_id"] == "R04" && n["via"] == "live" && n["output_matches"] == true));
    assert_eq!(res["metrics"]["run"]["verifier"], json!({ "source": "pass", "replay": "pass" }));
    assert_eq!(res["metrics"]["run"]["status"]["effects"]["live"], 0);
    // Nothing happened for real: no journal row, no receipt.
    assert_eq!((journal_rows(&t, &research), receipts(&t, &research)), (j0, c0), "replay is recorded_only");

    // Inline candidate graph: G03 asks for something else, so it re-generates; the rest is served.
    let mut g: Value = serde_json::from_str(general_json()).unwrap();
    g["graph_id"] = json!("general.task.v2");
    for n in g["nodes"].as_array_mut().unwrap() {
        if n["node_id"] == "G03" {
            n["extensions"]["x-title"] = json!("a shorter report");
        }
    }
    let (st, r) = replay(&t, json!({ "source_run_id": general, "overrides": { "graph_version": { "graph": g } } })).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{r}");
    let res = results(&t, r["id"].as_str().unwrap()).await;
    let via = |id: &str| res["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == id).map(|n| n["via"].clone());
    assert_eq!((via("G03"), via("G05")), (Some(json!("live")), Some(json!("recorded"))), "{}", res["nodes"]);
    // B1: G05 is an S1 node; the replay served the run's journaled decision.
    let g05 = res["nodes"].as_array().unwrap().iter().find(|n| n["node_id"] == "G05").unwrap();
    assert_eq!(g05["decision"]["choice"], "pass", "{g05}");
    assert_eq!(res["metrics"]["run"]["verifier"]["replay"], "pass");
    assert_ne!(res["verdict"], "diverged_unexpected", "{}", res["divergences"]);

    // decision_heads on the run's decisions.
    seed_e2e_decisions(&t.st.db, &general);
    let (st, r) = replay(&t, json!({ "source_run_id": general, "overrides": { "decision_heads": { "element": "head.lookup" } } })).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{r}");
    let res = results(&t, r["id"].as_str().unwrap()).await;
    assert_eq!(res["decisions"].as_array().unwrap().len(), 4);
    assert_eq!(res["verdict"], "diverged_expected", "the head changed d1: {}", res["divergences"]);
    assert_eq!(res["metrics"]["decisions"]["accuracy"].as_f64(), Some(1.0));

    // Batch over both runs; the admin's batch is promotion evidence.
    let (st, _, b) = call(&t.app, post("/v1/replays/batch", USER, None, json!({
        "runs": [general, research], "overrides": { "decision_heads": { "element": "head.lookup" }, "model_class": "mc.remote" } }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let batch: Value = serde_json::from_str(&b).unwrap();
    assert_eq!((batch["object"].as_str(), batch["status"].as_str()), (Some("replay_batch"), Some("completed")), "{batch}");
    assert_eq!(batch["metrics"]["runs"]["replayed"], 2, "{batch}");
    assert_eq!(batch["metrics"]["runs"]["verifier"]["pass_at_1"].as_f64(), Some(1.0));
    assert_eq!(batch["metrics"]["decisions"]["by_kind"]["element"]["labelled"], 3);
    let ev = &batch["evidence"][0];
    assert_eq!((ev["head"].as_str(), ev["kind"].as_str(), ev["evidence_id"].as_str()), (Some("head.lookup"), Some("element"), batch["id"].as_str()));
    let conn = t.st.db.connect().unwrap();
    assert_eq!(crate::agency_api::decisions::acceptance::latest(&conn, "head.lookup", "element").unwrap().unwrap().id, batch["id"].as_str().unwrap());
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/replays/batch/{}", batch["id"].as_str().unwrap()), USER)).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    // Every per-run replay of the batch is readable on its own.
    let rid = batch["replays"][0]["replay_id"].as_str().unwrap();
    let (st, _, b) = call(&t.app, get_req(&format!("/v1/replays/{rid}"), USER)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["batch_id"], batch["id"]);
    // Selector form; the admin acceptance check explains itself.
    let (st, _, b) = call(&t.app, post("/v1/replays/batch", USER, None, json!({ "selector": { "task_type": "GENERAL_TASK", "limit": 5 } }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["metrics"]["runs"]["replayed"], 1);
    let (st, _, b) = call(&t.app, post("/v1/decisions/heads/head.lookup/element/acceptance", USER, None, json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let x: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(x["replay"]["evidence"]["id"], batch["id"]);
    assert!(x["reason"].as_str().unwrap().contains("live bar not met"), "{x}");
    let (st, _, _) = call(&t.app, post("/v1/decisions/heads/head.lookup/element/acceptance", "someone", None, json!({}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!((journal_rows(&t, &research), receipts(&t, &research)), (j0, c0), "batch replay is recorded_only");
}

fn seed_e2e_decisions(db: &DbHandle, run: &str) {
    // Same shape as the unit seed, owned by the e2e user.
    let ids = ["a", "b", "c"];
    let opts = json!([{ "id": "a", "text": "A" }, { "id": "b", "text": "B" }, { "id": "c", "text": "C" }]);
    let put = |id: String, run: Option<&str>, ctx: &str, served: &str, outcome: Option<(&str, Option<&str>)>| {
        let key = flywheel::key_hash("element", ctx, None, &ids, &["A", "B", "C"]);
        dstore::insert(db, &dstore::Row {
            id: &id, owner: USER, created_at: &now(), kind: "element", session_id: run, task: None, context_hash: ctx, has_image: false,
            options: &opts, choice: Some(served), probs: &json!({}), confidence: 0.9, abstained: false, backend: "oracle",
            latency_ms: 500.0, attempts: &json!([{ "backend": "oracle", "detail": { "cost_usd": 0.002 } }]),
        }).unwrap();
        let s = flywheel::Shadow { decision_id: id.clone(), owner: USER.into(), kind: "element".into(), key, ids: ids.iter().map(|x| x.to_string()).collect(), has_image: false };
        flywheel::record_key(db, &s).unwrap();
        if let Some((status, label)) = outcome {
            dstore::set_outcome(db, &id, USER, status, label, None, &now()).unwrap();
            flywheel::on_outcome(db, &id, USER).unwrap();
        }
    };
    for i in 0..3 {
        put(format!("{run}_h{i}"), None, "ctxK", "b", Some(("success", None)));
    }
    put(format!("{run}_d1"), Some(run), "ctxK", "a", Some(("failure", Some("b"))));
    put(format!("{run}_d2"), Some(run), "ctxNew", "c", Some(("success", None)));
    put(format!("{run}_d3"), Some(run), "ctxSelf", "a", Some(("success", None)));
    put(format!("{run}_d4"), Some(run), "ctxK", "b", None);
}

// ── C5: replay and eval live calls count against the daily caps ─────────────

fn replay_usage(db: &DbHandle, org: &str, tokens: i64, micro_usd: i64) {
    db.connect().unwrap().execute(
        "INSERT INTO llm_usage_events (id, tenant_id, prompt_tokens, completion_tokens, cost_microdollars, status, source, surface)
         VALUES (?1, ?2, ?3, 0, ?4, 'ok', 'replay', 'replay')",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), org, tokens, micro_usd],
    ).unwrap();
}

#[tokio::test]
async fn replay_live_calls_count_against_the_org_daily_caps() {
    let t = setup().await;
    let s = AgencyStore::new(t.st.rails.ledger.clone());
    let org = "org_cap";
    let limits = guard::Limits::from_lookup(|k| match k {
        guard::DAILY_ENV => Some("off".into()),
        guard::ORG_DAILY_ENV => Some("usd=1".into()),
        _ => None,
    });
    let gate = sim::SpendGate::new(s.clone(), t.st.db.clone(), org, limits.clone());
    assert!(gate.admit().await.is_ok(), "nothing spent yet");
    // Replay spend of another org and of other sources does not count for this org.
    replay_usage(&t.st.db, "org_other", 10, 5_000_000);
    t.st.db.connect().unwrap().execute(
        "INSERT INTO llm_usage_events (id, tenant_id, cost_microdollars, status, source) VALUES ('x-gw', ?1, 9000000, 'ok', 'gateway')", [org]).unwrap();
    assert!(gate.admit().await.is_ok());
    let (g, o) = s.daily_spend_all(&t.st.db, org).await.unwrap();
    assert_eq!((g.tokens, o.tokens), (10, 0));
    assert!((g.usd - 5.0).abs() < 1e-9 && o.usd == 0.0);
    // This org's replay calls reach the cap: the next call is refused and the gate trips (fail closed).
    replay_usage(&t.st.db, org, 100, 600_000);
    replay_usage(&t.st.db, org, 100, 400_000);
    let (_, o) = s.daily_spend_all(&t.st.db, org).await.unwrap();
    assert_eq!(o.tokens, 200);
    let why = gate.admit().await.unwrap_err();
    assert!(why.contains("org daily usd cap"), "{why}");
    assert_eq!(gate.tripped().as_deref(), Some(why.as_str()));
    // The same spend now also blocks runs of the org (the executor and the
    // cap-lift check read daily_spend_all).
    assert_eq!(limits.daily_reached(&guard::Spend::default(), &o), Some(("org", "usd")));
    // Yesterday's replay spend does not count.
    let fresh = sim::SpendGate::new(s.clone(), t.st.db.clone(), "org_old", limits);
    t.st.db.connect().unwrap().execute(
        "INSERT INTO llm_usage_events (id, tenant_id, cost_microdollars, status, source, created_at) VALUES ('x-old', 'org_old', 9000000, 'ok', 'replay', '2000-01-01 00:00:00')", []).unwrap();
    assert!(fresh.admit().await.is_ok());
}
