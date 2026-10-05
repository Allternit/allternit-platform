//! WP10 end to end: BUG_FIX on a disposable TS/Vitest fixture. Lifecycle ->
//! router plans -> gate receipts on the signed chain -> verifier-owned
//! completion with before/after evidence -> close -> cassette -> replay with
//! zero divergence. The fix step is a deterministic scripted executor: no
//! model, no network.
use super::fixture::{Fixture, FIXED, IMPERFECT};
use super::*;
use crate::judge::completion::{load_policy, missing_evidence};
use crate::judge::policy::{effective_policy, CloseBy, PolicyOrigin};
use crate::kernel::lifecycle::{try_close, try_transition};
use crate::kernel::router::*;
use crate::kernel::{CloseOutcome, NodeState};
use crate::receipts::{ReceiptStore, ReceiptStoreOptions};
use crate::replay::{boundary_request_hash, record_cassette, Boundary, EffectsMode, ReplayStep, Replayer, StepOutcome, Verdict};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const RUN: &str = "run.wp10.bug_fix.e2e";
const POLICY_SCHEMA: &str = "allternit.kernel.PolicyReceiptV1";

fn pool_entry(id: &str, role: Role, mode: Mode, caps: &[&str]) -> PoolEntry {
    PoolEntry {
        schema_id: POOL_ENTRY_SCHEMA_ID.into(), schema_version: SCHEMA_VERSION.into(), backend_id: id.into(),
        cognitive_roles: vec![role], modes: vec![mode], capabilities: caps.iter().map(|c| c.to_string()).collect(),
        trust_tags: vec!["PUBLIC".into(), "INTERNAL".into()], confidence_estimate: 0.9, latency_ms: 100.0, cost: 0.1,
        residency: Residency::Warm, backbone_id: None, model_revision: None, runtime: None, quantization: None,
        layer_stop: None, readout_head_id: None, calibration_manifest_id: None, memory_mb: None, load_latency_ms: None,
        extensions: Some(Map::new()),
    }
}

/// Lifecycle driver: one attempt per (node, visit). Loops create new attempts.
#[derive(Default)]
struct Life { attempts: BTreeMap<String, NodeState>, order: Vec<String> }
impl Life {
    fn run(&mut self, node: &str, verified: bool) {
        let key = format!("{node}#{}", self.order.iter().filter(|k| k.starts_with(&format!("{node}#"))).count());
        let mut s = NodeState::Declared;
        for to in [NodeState::Admitted, NodeState::Ready, NodeState::Leased, NodeState::Spawned, NodeState::Running,
            NodeState::OutputReady, NodeState::Verifying] {
            s = try_transition(s, to).unwrap();
        }
        s = if verified {
            try_close(try_transition(s, NodeState::Committed).unwrap(), CloseOutcome::Committed).unwrap()
        } else {
            // Failed verification: replan, close FAILED; failure edge takes over.
            try_close(try_transition(s, NodeState::Replan).unwrap(), CloseOutcome::Failed).unwrap()
        };
        self.attempts.insert(key.clone(), s);
        self.order.push(key);
    }
}

/// Deterministic executor + gate: every effect goes through the gate's
/// `record_tool_effect` path and is logged as the replay step it must match.
struct Rig { fx: Fixture, rs: ReceiptStore, steps: Vec<ReplayStep>, seq: u32 }
impl Rig {
    fn effect(&mut self, tool: &str, class: &str, args: Value, exec: impl FnOnce(&mut Fixture) -> anyhow::Result<String>) -> String {
        self.seq += 1;
        let key = format!("wp10-e2e-{:04}", self.seq);
        let mut payload = args;
        payload["effect_class"] = json!(class);
        payload["idempotency_key"] = json!(key);
        let fx = &mut self.fx;
        let id = self.rs.record_tool_effect(RUN, tool, &payload, || exec(fx)).unwrap();
        self.steps.push(ReplayStep::tool(RUN, tool, &payload, class).unwrap().with_key(&key));
        id
    }
    fn vitest(&mut self, phase: &str, target: Option<&str>) -> (bool, String) {
        let id = self.effect("tool.vitest_run", "EXECUTE", json!({"phase": phase, "target": target}), |fx| {
            let o = fx.tests(target)?;
            Ok(format!("vitest:{phase}:{}", if o.status.success() { "PASS" } else { "FAIL" }))
        });
        (id.ends_with(":PASS"), id)
    }
    fn gate_allow(&mut self, node: &str) {
        let cs = self.rs.chain_store().unwrap();
        cs.append(json!({"envelope": {"schema_id": POLICY_SCHEMA, "schema_version": "1.0.0", "run_id": RUN, "node_id": node},
            "decision": "ALLOW", "write_set": ["fs:src/math.ts"]})).unwrap();
        self.steps.push(ReplayStep { boundary: Boundary::Policy, node_id: node.into(),
            request_hash: boundary_request_hash(Boundary::Policy, node, POLICY_SCHEMA).unwrap(),
            branch: Some("ALLOW".into()), result_hash: None, idempotency_key: None });
    }
    fn apply(&mut self, attempt: u32, text: &'static str) -> String {
        self.effect("tool.fs_write", "WORKSPACE_WRITE", json!({"path": "src/math.ts", "attempt": attempt, "content": text}), move |fx| {
            fx.write("src/math.ts", text)?;
            Ok(format!("patch:{attempt}:{}", crate::receipts::jcs::sha256_tagged(text.as_bytes())))
        })
    }
}

#[test]
fn wp10_e2e_bug_fix_fixture_lifecycle_router_gate_completion_replay() {
    let g = instantiate("task.wp10.fixture", &["fs:src/math.ts".into()]).unwrap();
    let d = tempfile::tempdir().unwrap();
    let rs = ReceiptStore::new(ReceiptStoreOptions { root_dir: Some(d.path().into()), receipts_dir: None, blobs_dir: None }).unwrap();
    let mut rig = Rig { fx: Fixture::new().unwrap(), rs, steps: vec![], seq: 0 };
    let mut life = Life::default();

    // Router: S0 -> primitive registry; S1 uncalibrated -> explicit S2 fallback node; S2 -> generative.
    let caps: Vec<String> = g.nodes.iter().filter_map(|n| n.capability_request.as_ref()?.get("capability")?.as_str().map(String::from)).collect();
    let caps: Vec<&str> = caps.iter().map(String::as_str).collect();
    let pool = StaticModelPool { entries: vec![
        pool_entry("be.s2.local", Role::S2, Mode::M5Generative, &caps),
        pool_entry("be.s3.local", Role::S3, Mode::M6DeepSolver, &caps),
    ] };
    let cfg = RouterConfig::default();
    let router = Router::new(&pool, &cfg);
    let ledger = BudgetLedger { remaining_cost_units: 100.0, remaining_wall_ms: None };
    let mut plans: BTreeMap<String, ExecutionPlan> = BTreeMap::new();
    let mut route = |id: &str| -> String {
        let node = g.node(id).unwrap();
        match router.route(node, &ledger) {
            Ok(p) => { let nid = id.to_string(); plans.insert(nid.clone(), p); nid }
            Err(RouteError::UncalibratedS1 { .. }) => {
                let fb = format!("F{}", &id[1..]);
                plans.insert(fb.clone(), router.route(g.node(&fb).unwrap(), &ledger).unwrap());
                fb
            }
            Err(e) => panic!("route {id}: {e:?}"),
        }
    };

    // N00–N10: intake, criteria, environment, packs, scope, index, retrieval, strategy.
    for id in ["N00", "N01", "N02", "N03", "N04", "N05", "N06", "N07", "N08", "N09", "N10"] {
        let ran = route(id);
        life.run(&ran, true);
    }
    // Reproduction: the target test fails before any change (evidence "before").
    let (ok, before) = rig.vitest("before", Some("math.test.ts"));
    assert!(!ok, "seeded bug must reproduce");
    let policy = load_policy("completion.bug_fix").unwrap();
    assert_eq!(missing_evidence(&policy, &[]).len(), 5, "no completion without evidence");

    // Repair loop. Scripted executor: attempt 1 is imperfect, attempt 2 is the fix.
    let mut target_pass = String::new();
    for (attempt, (gen_node, text)) in [("N11", IMPERFECT), ("N17", FIXED)].into_iter().enumerate() {
        let attempt = attempt as u32 + 1;
        life.run(&route(gen_node), true);
        rig.fx.write("src/math.ts", text).unwrap();
        let parsed = rig.fx.command(&["node", "parse.cjs"]).unwrap();
        rig.fx.checked(&["git", "checkout", "-q", "--", "src/math.ts"]).unwrap();
        life.run(&route("N12"), parsed.status.success());
        assert!(parsed.status.success());
        rig.gate_allow("N13");
        life.run(&route("N13"), true);
        rig.apply(attempt, text);
        life.run(&route("N14"), true);
        let (ok, id) = rig.vitest(&format!("target.{attempt}"), Some("math.test.ts"));
        life.run(&route("N15"), ok);
        if !ok {
            assert_eq!(attempt, 1);
            assert_eq!(error_class("TEST_ASSERTION"), "TEST_ASSERTION");
            life.run(&route("N16"), true);
        } else {
            assert_eq!(attempt, 2, "imperfect patch must not pass");
            target_pass = id;
        }
    }
    // Affected callers, requirements, diff review (write set respected), full-suite regression, typecheck.
    let (ok, affected) = rig.vitest("affected", Some("caller.test.ts"));
    assert!(ok);
    life.run(&route("N18"), ok);
    life.run(&route("N19"), true);
    let diff = rig.fx.checked(&["git", "diff", "--name-only"]).unwrap();
    assert_eq!(String::from_utf8_lossy(&diff.stdout).trim(), "src/math.ts");
    life.run(&route("N20"), true);
    let (ok, full) = rig.vitest("regression", None);
    assert!(ok);
    let tsc = rig.fx.command(&["node", "node_modules/typescript/bin/tsc", "-p", "."]).unwrap();
    assert!(tsc.status.success(), "{}", String::from_utf8_lossy(&tsc.stdout));

    // N21: verifier-owned completion. Agency origin forces close_by = verifier;
    // DONE needs one evidence ref per blocking criterion, each backed by a chain receipt.
    let ev = vec![crate::core::types::AllternitEvent {
        event_id: "e1".into(), ts: "2026-09-30T12:00:00Z".into(),
        actor: crate::core::types::Actor { r#type: crate::core::types::ActorType::User, id: "wp10".into() },
        scope: None, r#type: crate::judge::events::POLICY_SET.into(),
        payload: json!({"dag_id": RUN, "policy": {"origin": "agency", "completion_policy": "completion.bug_fix", "close_by": "any"}}),
        provenance: None,
    }];
    let eff = effective_policy(&ev, RUN, Some("N21"));
    assert_eq!((eff.close_by, eff.origin), (CloseBy::Verifier, Some(PolicyOrigin::Agency)));
    let evidence = vec![
        format!("target_tests_pass:receipt:{target_pass}"),
        format!("affected_tests_pass:receipt:{affected}"),
        format!("no_new_regressions:receipt:{full}"),
        "diff_review_accept:receipt:git-diff:src/math.ts".to_string(),
        "requirements_satisfied:receipt:vitest:acceptance".to_string(),
    ];
    assert_eq!(missing_evidence(&policy, &evidence[..3]), ["diff_review_accept", "requirements_satisfied"]);
    assert!(missing_evidence(&policy, &evidence).is_empty());
    let cs = rig.rs.chain_store().unwrap();
    let chain = cs.read_run(RUN).unwrap();
    for id in [&before, &target_pass, &affected, &full] {
        assert!(chain.iter().any(|r| r["external_ref"].as_str() == Some(id.as_str())), "no receipt for {id}");
    }
    assert!(cs.verify_chain(RUN).unwrap().ok);
    let ladder = verification_ladder();
    for step in ["parse", "typecheck", "target_test", "affected_tests", "diff_review", "regression"] {
        assert!(step_passes("PASS", ladder.iter().find(|s| s.step_id == step).unwrap().deterministic,
            &json!({"deterministic": true, "evidence_refs": [format!("receipt:{step}")]})));
    }
    life.run("N21", true);
    life.run(&route("N22"), true);
    life.run(&route("N23"), true);

    // Close: every attempt closed; N15 attempt 0 failed, the rest committed.
    assert!(life.attempts.values().all(|s| *s == NodeState::Closed));
    assert_eq!(life.order.iter().filter(|k| k.starts_with("N15#")).count(), 2);
    assert!(plans.contains_key("F01") && !plans.contains_key("N01"), "uncalibrated S1 fell back to S2");
    assert_eq!(plans["N00"].execution_mode, Mode::M0Deterministic);
    assert_eq!(plans["N11"].execution_mode, Mode::M5Generative);

    // Cassette + recorded-only replay: zero divergence, zero live effects.
    let commands = rig.fx.commands;
    let chain_len = chain.len();
    let cassette = record_cassette(&cs, RUN, Some(GRAPH_ID), 1).unwrap();
    let mut r = Replayer::new(&cs, cassette.clone(), "replay.wp10.e2e", EffectsMode::RecordedOnly).unwrap();
    for s in &rig.steps {
        assert!(matches!(r.step(s), StepOutcome::Recorded(_)), "refused {s:?}");
    }
    let rep = r.finish().unwrap();
    assert_eq!(rep.verdict, Verdict::Identical, "{rep:?}");
    assert!(rep.divergences.is_empty());
    assert_eq!(crate::replay::replay_report(&cs, cassette, None, "replay.wp10.self").unwrap().verdict, Verdict::Identical);
    assert_eq!(rig.fx.commands, commands, "replay ran no fixture command");
    assert_eq!(cs.read_run(RUN).unwrap().len(), chain_len, "replay appended nothing");
    assert_eq!(std::fs::read_to_string(rig.fx.path().join("src/math.ts")).unwrap(), FIXED);
}
