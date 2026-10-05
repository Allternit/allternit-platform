//! WP12 kernel-level conformance suite (Agency API alpha gate, CL-282; CL-001).
//!
//! Runs against the commrails kernel pieces on main. Test names start with a
//! group tag (`restart_`, `resume_`, `receipt_completion_`, `replay_`,
//! `fault_`, `rollback_`, `duplicate_`, `concurrency_`, `security_`,
//! `model_swap_`); `scripts/conformance-report.sh` maps groups to ledger rows.
//! API-level counterparts live in `allternit-api/tests/agency_conformance.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use allternit_factory_engine::drive::hooks::NoHooks;
use allternit_factory_engine::drive::{DriveOptions, Driver, ATTEMPT_FINISHED, ATTEMPT_STARTED};
use allternit_factory_engine::egress::{host_is_forbidden_literal, is_public_ip};
use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::judge::policy::{CloseBy, JudgePolicy, PolicyOrigin, VerifyMode};
use allternit_factory_engine::judge::{StubJudge, ToolDecision, ToolDecisionSource};
use allternit_factory_engine::kernel::graph::{validate, ComputeGraph};
use allternit_factory_engine::kernel::registry::PrimitiveRegistry;
use allternit_factory_engine::kernel::router::{
    BudgetLedger, ExecutionPlan, Mode, PoolEntry, RouteError, Role, Router, RouterConfig,
    StaticModelPool,
};
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::receipts::chain::{ChainStore, EffectContext, EffectOutcome, EffectRequest};
use allternit_factory_engine::receipts::jcs::{hash_value, sha256_tagged};
use allternit_factory_engine::receipts::sign::ReceiptSigner;
use allternit_factory_engine::replay::{
    record_cassette, replay_report, Boundary, EffectsMode, ReplayStep, Replayer, StepOutcome, Verdict,
};
use allternit_factory_engine::templates::RETRY_SAFE_LABEL;
use allternit_factory_engine::work::{project_dag, DagState};
use allternit_factory_engine::{
    Actor, ActorType, AllternitEvent, Gate, GateError, GateOptions, Leases, Ledger, LedgerQuery,
    ReceiptStore, ReceiptStoreOptions,
};
use serde_json::{json, Value};
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix("conf-").tempdir_in(base).unwrap()
}

async fn build_gate(root: &Path) -> (Arc<Ledger>, Arc<Gate>) {
    std::env::set_var("ALLTERNIT_COMMRAILS_BIN", env!("CARGO_BIN_EXE_allternit-factory"));
    let ledger = Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.to_path_buf()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }));
    let leases = Arc::new(
        Leases::new(LeasesOptions {
            root_dir: Some(root.to_path_buf()),
            leases_dir: Some(PathBuf::from(".allternit/leases")),
            event_sink: Some(ledger.clone()),
            actor_id: Some("gate".to_string()),
            auto_renewal_enabled: false,
            auto_renewal_threshold_seconds: 300,
            auto_renewal_interval_seconds: 60,
            auto_renewal_extend_seconds: 600,
        })
        .await
        .unwrap(),
    );
    let receipts = Arc::new(
        ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.to_path_buf()),
            receipts_dir: Some(PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(PathBuf::from(".allternit/blobs")),
        })
        .unwrap(),
    );
    let gate = Arc::new(Gate::new(GateOptions {
        ledger: ledger.clone(),
        leases,
        receipts,
        index: None,
        vault: None,
        oauth_vault: None,
        root_dir: Some(root.to_path_buf()),
        actor_id: Some("gate".to_string()),
        strict_provenance: None,
        visual_provider: None,
        visual_config: None,
    }));
    (ledger, gate)
}

fn with_stub(gate: Arc<Gate>, node: &str, tool: &str) -> Gate {
    let g = Arc::try_unwrap(gate).ok().expect("sole gate owner");
    g.with_judge(Arc::new(StubJudge::new(node, tool)), Duration::from_millis(300), Duration::from_millis(300))
}

fn user(id: &str) -> Actor {
    Actor { r#type: ActorType::User, id: id.into() }
}
fn agent(id: &str) -> Actor {
    Actor { r#type: ActorType::Agent, id: id.into() }
}
fn gate_err(err: &anyhow::Error) -> &GateError {
    GateError::from_anyhow(err).unwrap_or_else(|| panic!("expected GateError, got {err:#}"))
}

fn task(id: &str, executor: Option<&str>) -> Mutation {
    Mutation::CreateNode {
        node_id: id.into(),
        node_kind: "task".into(),
        title: id.to_uppercase(),
        parent_node_id: None,
        execution_mode: "shared".into(),
        description: Some("Write a haiku about rails.".into()),
        executor: executor.map(String::from),
    }
}

/// Plan a DAG from `mutations` (CreateNode parents are set to the root).
async fn plan(gate: &Gate, mutations: Vec<Mutation>) -> String {
    let (_, dag_id, root) = gate.plan_new("conformance goal", None).await.unwrap();
    let ms = mutations
        .into_iter()
        .map(|m| match m {
            Mutation::CreateNode { node_id, node_kind, title, execution_mode, description, executor, .. } => {
                Mutation::CreateNode {
                    node_id, node_kind, title, parent_node_id: Some(root.clone()), execution_mode, description, executor,
                }
            }
            o => o,
        })
        .collect();
    gate.plan_refine(&dag_id, "conformance nodes", "test", ms).await.unwrap();
    dag_id
}

async fn dag(ledger: &Ledger, dag_id: &str) -> DagState {
    let events: Vec<AllternitEvent> = ledger
        .query(LedgerQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .collect();
    project_dag(&events, dag_id)
}

async fn events_of(ledger: &Ledger, ty: &str) -> Vec<AllternitEvent> {
    ledger.query(LedgerQuery { r#type: Some(ty.into()), ..Default::default() }).await.unwrap()
}

fn agency() -> JudgePolicy {
    JudgePolicy {
        origin: Some(PolicyOrigin::Agency),
        completion_policy: Some("completion.bug_fix".into()),
        ..Default::default()
    }
}

fn evidence(criteria: &[&str]) -> Vec<String> {
    criteria.iter().map(|c| format!("{c}:receipt:r_{c}")).collect()
}

const ALL_EVIDENCE: [&str; 5] =
    ["target_tests_pass", "affected_tests_pass", "no_new_regressions", "diff_review_accept", "requirements_satisfied"];

fn stub_harness(root: &Path, name: &str, body: &str) {
    let dir = root.join("stubs");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\nprompt_file=\"$1\"\nnode=\"$2\"\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn write_drive_config(root: &Path) {
    let claude = root.join("stubs/claude");
    let cfg = json!({
        "poll_interval_ms": 100, "min_free_mem_mb": 0, "max_load_per_cpu": 100000.0,
        "harnesses": { "claude": { "argv": [claude, "{prompt_file}", "{node_id}"] } }
    });
    let dir = root.join(".allternit/drive");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
}

async fn try_drive(root: &Path, ledger: &Arc<Ledger>, gate: &Arc<Gate>, dag_id: &str) -> anyhow::Result<()> {
    let opts = DriveOptions { dag_id: dag_id.into(), ..Default::default() };
    let mut d = Driver::new(root.to_path_buf(), ledger.clone(), Some(gate.clone()), opts, Arc::new(NoHooks)).unwrap();
    tokio::time::timeout(Duration::from_secs(90), d.run(std::future::pending::<()>()))
        .await
        .expect("drive did not finish")
        .map(|_| ())
}

async fn drive(root: &Path, ledger: &Arc<Ledger>, gate: &Arc<Gate>, dag_id: &str) {
    try_drive(root, ledger, gate, dag_id).await.unwrap();
}

/// An open attempt whose session is gone: what a killed executor leaves behind.
async fn killed_attempt(ledger: &Ledger, gate: &Gate, root: &Path, dag_id: &str, node_id: &str) {
    let wih = gate.wih_pickup(dag_id, node_id, "drive-claude").await.unwrap();
    ledger
        .append(AllternitEvent {
            event_id: String::new(),
            ts: String::new(),
            actor: Actor { r#type: ActorType::Agent, id: "drive".into() },
            scope: None,
            r#type: ATTEMPT_STARTED.to_string(),
            payload: json!({
                "dag_id": dag_id, "node_id": node_id, "wih_id": wih, "attempt_id": format!("{wih}-a1"),
                "attempt": 1, "executor": "ao:claude", "harness": "claude",
                "slug": format!("gone-{node_id}"), "run_dir": root.join(".allternit/drive/runs/gone"),
                "timeout_seconds": 60
            }),
            provenance: None,
        })
        .await
        .unwrap();
}

// receipt-chain helpers
fn chain_store() -> (TempDir, ChainStore) {
    let d = tempfile::tempdir().unwrap();
    let s = ChainStore::new(d.path(), ReceiptSigner::generate()).unwrap();
    (d, s)
}
fn cx(run: &str, node: &str) -> EffectContext {
    EffectContext {
        run_id: run.into(), session_id: "s1".into(), task_id: "t1".into(), node_id: Some(node.into()),
        trace_id: "tr1".into(), state_version: 1, producer_id: "commrails".into(), policy_decision_id: "dec1".into(),
    }
}
fn args(n: u32) -> Value {
    json!({"path": format!("f{n}.txt"), "content": n})
}
fn req(n: u32, key: &str) -> EffectRequest {
    EffectRequest {
        action_id: format!("act{n}"), tool_id: "tool.fs_write".into(), args_hash: hash_value(&args(n)).unwrap(),
        idempotency_key: key.into(), effect_class: "WORKSPACE_WRITE".into(), target: None,
    }
}
fn rstep(n: u32) -> ReplayStep {
    ReplayStep::tool(&format!("node{n}"), "tool.fs_write", &args(n), "WORKSPACE_WRITE").unwrap()
}
/// A recorded run: one policy decision, then three side-effecting tool calls.
fn live_run(s: &ChainStore, run: &str) -> u32 {
    let mut calls = 0;
    s.append(json!({"envelope": {"schema_id": "allternit.kernel.PolicyReceiptV1", "schema_version": "1.0.0",
        "run_id": run, "node_id": "gate"}, "decision": "ALLOW"}))
        .unwrap();
    for n in 0..3 {
        let o = s
            .run_effect_once(&cx(run, &format!("node{n}")), &req(n, &format!("idem-key-{n}")), || {
                calls += 1;
                Ok((sha256_tagged(format!("out{n}").as_bytes()), Some(format!("ext{n}"))))
            })
            .unwrap();
        assert!(matches!(o, EffectOutcome::Executed(_)));
    }
    calls
}
fn policy_step() -> ReplayStep {
    ReplayStep {
        boundary: Boundary::Policy, node_id: "gate".into(),
        request_hash: allternit_factory_engine::replay::boundary_request_hash(Boundary::Policy, "gate", "allternit.kernel.PolicyReceiptV1").unwrap(),
        branch: Some("ALLOW".into()), result_hash: None, idempotency_key: None,
    }
}

// router helpers
fn pool_entry(id: &str, role: Role, mode: Mode, cap: &str, conf: f64, cost: f64, vendor_ref: &str) -> PoolEntry {
    serde_json::from_value(json!({
        "schema_id": "allternit.kernel.ModelPoolEntryV1", "schema_version": "1.0.0",
        "backend_id": id, "cognitive_roles": [role], "modes": [mode], "capabilities": [cap],
        "trust_tags": ["PUBLIC", "INTERNAL"], "confidence_estimate": conf, "latency_ms": 400.0, "cost": cost,
        "residency": "WARM", "backbone_id": vendor_ref, "model_revision": vendor_ref,
        "extensions": {"x-model_ref": vendor_ref}
    }))
    .unwrap()
}
fn graph_node(id: &str, role: &str, cap: &str) -> allternit_factory_engine::kernel::graph::GraphNode {
    serde_json::from_value(json!({
        "node_id": id, "primitive_id": "generate.patch", "node_kind": "COMPUTE", "cognitive_role": role,
        "inputs": [], "outputs": [], "read_set": [], "write_set": [], "lock_scope": [],
        "on_failure": {"strategy": "FAIL"},
        "capability_request": {"schema_id": "allternit.kernel.CapabilityRequestV1", "schema_version": "1.0.0",
            "capability": cap, "modality": "CODE", "latency_class": "NORMAL", "trust_requirement": "PUBLIC", "budget": {}}
    }))
    .unwrap()
}
const VENDOR: &[&str] = &[
    "openai", "anthropic", "claude", "codex", "gpt", "gemini", "llama", "mistral", "deepseek", "qwen", "sonnet", "kimi", "grok",
];
fn assert_no_vendor(label: &str, text: &str) {
    let lower = text.to_ascii_lowercase();
    for v in VENDOR {
        assert!(!lower.contains(v), "{label}: vendor name {v:?} found");
    }
}

// ------------------------------------------------------------------ restart

#[tokio::test]
async fn restart_run_state_survives_process_restart() {
    let tmp = test_root();
    let dag_id = {
        let (ledger, gate) = build_gate(tmp.path()).await;
        let dag_id = plan(&gate, vec![task("rs_a", None), task("rs_b", None)]).await;
        gate.wih_pickup(&dag_id, "rs_a", "worker-1").await.unwrap();
        assert_eq!(dag(&ledger, &dag_id).await.nodes.len(), 3);
        dag_id
    };
    // "Restart": every in-memory object is dropped; rebuild from the durable root only.
    let (ledger2, gate2) = build_gate(tmp.path()).await;
    let d = dag(&ledger2, &dag_id).await;
    assert!(d.nodes.contains_key("rs_a") && d.nodes.contains_key("rs_b"), "plan survived");
    // The open lease on rs_a survived too: a second pickup is fenced out.
    assert!(gate2.wih_pickup(&dag_id, "rs_a", "worker-2").await.is_err());
    // And the untouched node is still runnable.
    gate2.wih_pickup(&dag_id, "rs_b", "worker-2").await.unwrap();
}

#[test]
fn restart_receipt_chain_reopens_and_verifies() {
    let (d, s) = chain_store();
    live_run(&s, "run1");
    let head = s.read_run("run1").unwrap().len();
    drop(s);
    let s2 = ChainStore::open(d.path()).unwrap();
    assert_eq!(s2.read_run("run1").unwrap().len(), head);
    assert!(s2.verify_chain("run1").unwrap().ok, "chain verifies after reopen");
    // Appending continues the same chain rather than forking it.
    s2.append(json!({"envelope": {"schema_id": "allternit.kernel.PolicyReceiptV1", "schema_version": "1.0.0",
        "run_id": "run1", "node_id": "gate2"}, "decision": "ALLOW"})).unwrap();
    assert!(s2.verify_chain("run1").unwrap().ok);
    assert_eq!(s2.read_run("run1").unwrap().len(), head + 1);
}

// ------------------------------------------------------------------ resume

#[tokio::test]
async fn resume_killed_retry_safe_node_restarts_to_done() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    stub_harness(&root, "claude", "echo ok");
    write_drive_config(&root);
    let (ledger, gate) = build_gate(&root).await;
    let dag_id = plan(&gate, vec![task("rz_a", Some("ao:claude")), Mutation::AddLabel { node_id: "rz_a".into(), label: RETRY_SAFE_LABEL.into() }]).await;
    killed_attempt(&ledger, &gate, &root, &dag_id, "rz_a").await;
    drive(&root, &ledger, &gate, &dag_id).await;
    assert_eq!(dag(&ledger, &dag_id).await.nodes["rz_a"].status, "DONE");
    assert_eq!(events_of(&ledger, ATTEMPT_FINISHED).await[0].payload["outcome"], "interrupted");
}

#[tokio::test]
async fn resume_killed_non_idempotent_node_fails_closed_to_a_human() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    stub_harness(&root, "claude", "echo should-not-run > \"$0.ran\"");
    write_drive_config(&root);
    let (ledger, gate) = build_gate(&root).await;
    let dag_id = plan(&gate, vec![task("rz_b", Some("ao:claude"))]).await;
    killed_attempt(&ledger, &gate, &root, &dag_id, "rz_b").await;
    drive(&root, &ledger, &gate, &dag_id).await;
    assert_ne!(dag(&ledger, &dag_id).await.nodes["rz_b"].status, "DONE", "never silently re-executed");
    assert!(!root.join("stubs/claude.ran").exists());
}

// ---------------------------------------------- receipt-backed completion

#[tokio::test]
async fn receipt_completion_worker_self_close_is_a_proposal_not_done() {
    let tmp = test_root();
    let (ledger, gate) = build_gate(tmp.path()).await;
    let gate = with_stub(gate, "accomplished", "allow");
    let dag_id = plan(&gate, vec![task("rc_a", None)]).await;
    gate.set_judge_policy(&dag_id, None, agency(), &user("eoj")).await.unwrap();
    let wih = gate.wih_pickup(&dag_id, "rc_a", "builder-1").await.unwrap();
    let err = gate.wih_close_as(&wih, "DONE", &evidence(&ALL_EVIDENCE), None, Some(&agent("builder-1"))).await.unwrap_err();
    assert_eq!(gate_err(&err).code, "completion_proposed");
    assert_ne!(dag(&ledger, &dag_id).await.nodes["rc_a"].status, "DONE");
    assert!(!events_of(&ledger, "WIHClosedSigned").await.iter().any(|_| true));
}

#[tokio::test]
async fn receipt_completion_missing_evidence_never_reaches_done() {
    let tmp = test_root();
    let (ledger, gate) = build_gate(tmp.path()).await;
    let gate = with_stub(gate, "accomplished", "allow");
    let dag_id = plan(&gate, vec![task("rc_b", None)]).await;
    gate.set_judge_policy(&dag_id, None, agency(), &user("eoj")).await.unwrap();
    let wih = gate.wih_pickup(&dag_id, "rc_b", "builder-1").await.unwrap();
    // Every strict subset of the required evidence must fail to close.
    for skip in 0..ALL_EVIDENCE.len() {
        let partial: Vec<&str> = ALL_EVIDENCE.iter().enumerate().filter(|(i, _)| *i != skip).map(|(_, c)| *c).collect();
        let _ = gate.wih_close_as(&wih, "DONE", &evidence(&partial), None, Some(&agent("verifier-1"))).await;
        assert_ne!(dag(&ledger, &dag_id).await.nodes["rc_b"].status, "DONE", "closed without {}", ALL_EVIDENCE[skip]);
    }
}

#[tokio::test]
async fn receipt_completion_verifier_plus_full_evidence_is_done() {
    let tmp = test_root();
    let (ledger, gate) = build_gate(tmp.path()).await;
    let gate = with_stub(gate, "accomplished", "allow");
    let dag_id = plan(&gate, vec![task("rc_c", None)]).await;
    gate.set_judge_policy(&dag_id, None, agency(), &user("eoj")).await.unwrap();
    let wih = gate.wih_pickup(&dag_id, "rc_c", "builder-1").await.unwrap();
    let out = gate.wih_close_as(&wih, "DONE", &evidence(&ALL_EVIDENCE), None, Some(&agent("verifier-1"))).await.unwrap();
    assert_eq!(out.node_status, "DONE");
    assert_eq!(dag(&ledger, &dag_id).await.nodes["rc_c"].status, "DONE");
}

#[tokio::test]
async fn receipt_completion_agency_policy_cannot_be_weakened() {
    let tmp = test_root();
    let (_, gate) = build_gate(tmp.path()).await;
    let dag_id = plan(&gate, vec![task("rc_d", None)]).await;
    gate.set_judge_policy(&dag_id, None, agency(), &user("eoj")).await.unwrap();
    let weak = JudgePolicy { verify: Some(VerifyMode::Off), close_by: Some(CloseBy::Any), ..Default::default() };
    for who in [agent("builder-1"), user("client")] {
        assert!(gate.set_judge_policy(&dag_id, None, weak.clone(), &who).await.is_err());
    }
    assert_eq!(gate.judge_policy(&dag_id, None).await.unwrap().verify, VerifyMode::Judge);
}

// ------------------------------------------------------------------ replay

#[test]
fn replay_recorded_run_has_zero_divergence_and_no_live_effects() {
    let (_d, s) = chain_store();
    let calls = live_run(&s, "run1");
    assert_eq!(calls, 3);
    let len = s.read_run("run1").unwrap().len();
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    let mut r = Replayer::new(&s, c, "replay1", EffectsMode::RecordedOnly).unwrap();
    assert!(matches!(r.step(&policy_step()), StepOutcome::Recorded(_)));
    for n in 0..3 {
        let StepOutcome::Recorded(res) = r.step(&rstep(n).with_key(&format!("idem-key-{n}"))) else { panic!("refused") };
        assert_eq!(res.result_hash, sha256_tagged(format!("out{n}").as_bytes()));
    }
    let rep = r.finish().unwrap();
    assert_eq!(rep.verdict, Verdict::Identical);
    assert!(rep.divergences.is_empty());
    assert_eq!(s.read_run("run1").unwrap().len(), len, "replay appended nothing");
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    assert_eq!(replay_report(&s, c, None, "self").unwrap().verdict, Verdict::Identical);
}

#[test]
fn replay_refuses_unrecorded_effects_and_detects_tampering() {
    let (_d, s) = chain_store();
    live_run(&s, "run1");
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    let mut r = Replayer::new(&s, c.clone(), "r", EffectsMode::RecordedOnly).unwrap();
    r.step(&policy_step());
    assert!(matches!(r.step(&rstep(99)), StepOutcome::Refused(_)), "an effect the recording never made must not run");
    assert_eq!(r.finish().unwrap().verdict, Verdict::UnexpectedDivergence);
    let mut forged = c;
    forged.entries[1].recorded_result_hash = sha256_tagged(b"lie");
    assert_eq!(replay_report(&s, forged, None, "r").unwrap().verdict, Verdict::UnexpectedDivergence);
}

// ------------------------------------------------------------------ fault

#[test]
fn fault_torn_receipt_write_is_detected_by_chain_verification() {
    let (d, s) = chain_store();
    live_run(&s, "run1");
    assert!(s.verify_chain("run1").unwrap().ok);
    let n = s.read_run("run1").unwrap().len() as u64;
    let p = d.path().join(format!("_chains/run1/{:010}.json", n - 1));
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() / 2]).unwrap(); // torn: half a receipt hit disk
    let broken = s.verify_chain("run1").map(|r| !r.ok).unwrap_or(true);
    assert!(broken, "a torn write must never verify");
}

#[test]
fn fault_crash_mid_effect_requires_reconciliation_not_retry() {
    let (_d, s) = chain_store();
    // Executor killed after INTENDED was durably written and before COMMITTED.
    s.record_effect(&cx("run1", "node0"), &req(0, "idem-key-0"), "INTENDED", None, None, None).unwrap();
    let mut ran = false;
    let r = s.run_effect_once(&cx("run1", "node0"), &req(0, "idem-key-0"), || {
        ran = true;
        Ok((sha256_tagged(b"x"), None))
    });
    assert!(r.is_err(), "unresolved INTENDED must fail closed");
    assert!(!ran, "the effect must not run twice");
}

#[test]
fn fault_budget_exhaustion_fails_closed() {
    let pool = StaticModelPool { entries: vec![pool_entry("be.a", Role::S2, Mode::M5Generative, "cap.code.edit", 0.8, 1.0, "vendor-x")] };
    let cfg = RouterConfig::default();
    let n = graph_node("n1", "S2", "cap.code.edit");
    let ok = Router::new(&pool, &cfg).route(&n, &BudgetLedger { remaining_cost_units: 10.0, remaining_wall_ms: None });
    assert!(ok.is_ok());
    let err = Router::new(&pool, &cfg).route(&n, &BudgetLedger { remaining_cost_units: 0.0, remaining_wall_ms: None }).unwrap_err();
    assert!(matches!(err, RouteError::BudgetExhausted(_) | RouteError::NoEligibleBackend { .. } | RouteError::PolicyRejected { .. }), "{err:?}");
    let empty = StaticModelPool { entries: vec![] };
    assert!(Router::new(&empty, &cfg).route(&n, &BudgetLedger { remaining_cost_units: 10.0, remaining_wall_ms: None }).is_err(), "empty pool fails closed");
}

// ---------------------------------------------------------------- rollback

#[tokio::test]
async fn rollback_restores_files_from_the_wih_backup() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    let (_, gate) = build_gate(&root).await;
    let dag_id = plan(&gate, vec![task("rb_a", None)]).await;
    let wih = gate.wih_pickup(&dag_id, "rb_a", "worker-1").await.unwrap();
    std::fs::write(root.join("app.txt"), "original").unwrap();
    let b = root.join(".allternit/backups").join(format!("{wih}_1700000000"));
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("app.txt"), "original").unwrap();
    std::fs::write(root.join("app.txt"), "bad patch").unwrap();
    gate.rollback_wih(&wih).await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join("app.txt")).unwrap(), "original");
    // No backup, no rollback claim.
    assert!(gate.rollback_wih("wih_none").await.is_err());
}

#[test]
fn rollback_failed_effect_may_be_retried_but_committed_effect_is_final() {
    let (_d, s) = chain_store();
    let o = s.run_effect_once(&cx("run1", "n"), &req(1, "idem-key-1"), || anyhow::bail!("boom")).unwrap();
    let EffectOutcome::Executed(r) = o else { panic!() };
    assert_eq!(r["status"], "FAILED");
    let o = s.run_effect_once(&cx("run1", "n"), &req(1, "idem-key-1"), || Ok((sha256_tagged(b"ok"), None))).unwrap();
    let EffectOutcome::Executed(r) = o else { panic!() };
    assert_eq!(r["status"], "COMMITTED");
    assert!(s.verify_chain("run1").unwrap().ok, "compensation history stays chain-valid");
}

// -------------------------------------------------------- duplicate goal

#[test]
fn duplicate_effect_with_same_idempotency_key_executes_once() {
    let (_d, s) = chain_store();
    let mut runs = 0;
    for i in 0..3 {
        let o = s.run_effect_once(&cx("run1", "n"), &req(1, "idem-key-1"), || {
            runs += 1;
            Ok((sha256_tagged(b"ok"), Some("ext".into())))
        }).unwrap();
        assert_eq!(matches!(o, EffectOutcome::Replayed(_)), i > 0);
    }
    assert_eq!(runs, 1);
}

#[tokio::test]
async fn duplicate_close_of_a_done_node_is_not_a_second_completion() {
    let tmp = test_root();
    let (ledger, gate) = build_gate(tmp.path()).await;
    let gate = with_stub(gate, "accomplished", "allow");
    let dag_id = plan(&gate, vec![task("dp_a", None)]).await;
    gate.set_judge_policy(&dag_id, None, agency(), &user("eoj")).await.unwrap();
    let wih = gate.wih_pickup(&dag_id, "dp_a", "builder-1").await.unwrap();
    gate.wih_close_as(&wih, "DONE", &evidence(&ALL_EVIDENCE), None, Some(&agent("verifier-1"))).await.unwrap();
    let again = gate.wih_close_as(&wih, "DONE", &evidence(&ALL_EVIDENCE), None, Some(&agent("verifier-1"))).await;
    let _ = again; // either refused or idempotent; never a second signed close
    assert_eq!(events_of(&ledger, "WIHClosedSigned").await.len(), 1);
}

// ------------------------------------------------------------ concurrency

#[tokio::test]
async fn concurrency_two_pickups_of_one_node_are_fenced_to_one_winner() {
    let tmp = test_root();
    let (_, gate) = build_gate(tmp.path()).await;
    let dag_id = plan(&gate, vec![task("cc_a", None)]).await;
    let (g1, g2) = (gate.clone(), gate.clone());
    let (d1, d2) = (dag_id.clone(), dag_id.clone());
    let (a, b) = tokio::join!(
        tokio::spawn(async move { g1.wih_pickup(&d1, "cc_a", "driver-1").await.is_ok() }),
        tokio::spawn(async move { g2.wih_pickup(&d2, "cc_a", "driver-2").await.is_ok() }),
    );
    let winners = [a.unwrap(), b.unwrap()].iter().filter(|w| **w).count();
    assert!(winners <= 1, "lease fencing: never two owners of one node");
    assert!(winners == 1 || gate.wih_pickup(&dag_id, "cc_a", "driver-3").await.is_ok(), "someone can still take it");
}

#[tokio::test]
async fn concurrency_two_drivers_on_one_dag_run_each_node_once() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    stub_harness(&root, "claude", "sleep 1; echo done");
    write_drive_config(&root);
    let (ledger, gate) = build_gate(&root).await;
    let dag_id = plan(&gate, vec![task("cd_a", Some("ao:claude")), task("cd_b", Some("ao:claude")), task("cd_c", Some("ao:claude"))]).await;
    let (r1, r2) = tokio::join!(
        try_drive(&root, &ledger, &gate, &dag_id),
        try_drive(&root, &ledger, &gate, &dag_id)
    );
    // The DAG lock fences the second driver out; the first finishes the run.
    let (ok, err): (Vec<_>, Vec<_>) = [r1, r2].into_iter().partition(|r| r.is_ok());
    assert_eq!(ok.len(), 1, "exactly one driver owns the DAG");
    assert!(err[0].as_ref().unwrap_err().to_string().contains("already driving"));
    let d = dag(&ledger, &dag_id).await;
    for n in ["cd_a", "cd_b", "cd_c"] {
        assert_eq!(d.nodes[n].status, "DONE", "{n}");
    }
    let started = events_of(&ledger, ATTEMPT_STARTED).await;
    for n in ["cd_a", "cd_b", "cd_c"] {
        assert_eq!(started.iter().filter(|e| e.payload["node_id"] == n).count(), 1, "{n} attempted exactly once");
    }
}

// --------------------------------------------------------------- security

#[tokio::test]
async fn security_deny_beats_bypass_and_a_permissive_judge() {
    let tmp = test_root();
    let (_, gate) = build_gate(tmp.path()).await;
    let gate = with_stub(gate, "accomplished", "allow");
    let dag_id = plan(&gate, vec![task("sc_a", None)]).await;
    let wih = gate.wih_pickup(&dag_id, "sc_a", "agent-x").await.unwrap();
    // Un-signed WIH: Gate 2 denies even though the judge would allow.
    let v = gate.judge_tool_call(&wih, "bash", Some("ls"), &[]).await.unwrap();
    assert_eq!((v.decision, v.source), (ToolDecision::Deny, ToolDecisionSource::Gate2));
    gate.wih_sign_open(&wih, "sig").await.unwrap();
    // Harness in auto-approve (bypass) mode: the hard floor still denies.
    let v = gate.judge_tool_call(&wih, "bash", Some("rm -rf /"), &[]).await.unwrap();
    assert_eq!((v.decision, v.source), (ToolDecision::Deny, ToolDecisionSource::HardRule));
}

#[test]
fn security_egress_guard_blocks_private_and_metadata_addresses() {
    for h in ["169.254.169.254", "127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.1", "100.64.0.1", "::1", "::ffff:169.254.169.254", "[::1]", "localhost", "printer.local"] {
        assert!(host_is_forbidden_literal(h), "{h} must be forbidden");
    }
    for h in ["1.1.1.1", "8.8.8.8", "example.com"] {
        assert!(!host_is_forbidden_literal(h), "{h} must be allowed");
    }
    assert!(!is_public_ip("169.254.169.254".parse().unwrap()));
    assert!(is_public_ip("1.1.1.1".parse().unwrap()));
}

#[test]
fn security_no_vendor_names_in_contract_data_or_plans() {
    let abi = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/Contracts/kernel/v1");
    for dir in ["data", "conformance/examples/valid"] {
        for e in std::fs::read_dir(format!("{abi}/{dir}")).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                let t = std::fs::read_to_string(e.path()).unwrap();
                assert_no_vendor(&e.path().display().to_string(), &t);
            }
        }
    }
}

#[test]
fn security_registry_identity_never_leaks_into_plans_or_receipts() {
    let pool = StaticModelPool {
        entries: vec![pool_entry("be.a", Role::S2, Mode::M5Generative, "cap.code.edit", 0.8, 0.1, "openai/gpt via anthropic claude llama")],
    };
    let plan = Router::new(&pool, &RouterConfig::default())
        .route(&graph_node("n1", "S2", "cap.code.edit"), &BudgetLedger { remaining_cost_units: 5.0, remaining_wall_ms: None })
        .unwrap();
    assert_no_vendor("plan", &serde_json::to_string(&plan).unwrap());
    let (_d, s) = chain_store();
    live_run(&s, "run1");
    assert_no_vendor("receipts", &serde_json::to_string(&s.read_run("run1").unwrap()).unwrap());
}

// -------------------------------------------------------- model swap (CL-001)

fn shape(p: &ExecutionPlan) -> (Role, String, Mode, f64) {
    (p.cognitive_role, p.capability_id.clone(), p.execution_mode, p.confidence_floor)
}

#[test]
fn model_swap_does_not_change_plan_shape_or_graph() {
    let nodes = [graph_node("n_edit", "S2", "cap.code.edit"), graph_node("n_solve", "S3", "cap.code.edit")];
    let pool_a = StaticModelPool { entries: vec![
        pool_entry("be.alpha", Role::S2, Mode::M5Generative, "cap.code.edit", 0.8, 0.2, "vendor-a-1"),
        pool_entry("be.alpha.deep", Role::S3, Mode::M6DeepSolver, "cap.code.edit", 0.9, 1.0, "vendor-a-2"),
    ]};
    let pool_b = StaticModelPool { entries: vec![
        pool_entry("be.omega", Role::S2, Mode::M5Generative, "cap.code.edit", 0.7, 0.05, "vendor-b-1"),
        pool_entry("be.omega.deep", Role::S3, Mode::M6DeepSolver, "cap.code.edit", 0.95, 3.0, "vendor-b-2"),
    ]};
    let cfg = RouterConfig::default();
    let budget = BudgetLedger { remaining_cost_units: 50.0, remaining_wall_ms: None };
    for n in &nodes {
        let a = Router::new(&pool_a, &cfg).route(n, &budget).unwrap();
        let b = Router::new(&pool_b, &cfg).route(n, &budget).unwrap();
        assert_ne!(a.backend_id, b.backend_id, "the backend really was swapped");
        assert_eq!(shape(&a), shape(&b), "swapping the model changed the loop shape for {}", n.node_id);
    }
    // The graph never names a model, so no swap can edit it (invariant I7).
    let g = ComputeGraph::from_json(&std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"), "/../../spec/Contracts/kernel/v1/conformance/examples/valid/ComputeGraphIRV1.json")).unwrap()).unwrap();
    let violations = validate(&g, PrimitiveRegistry::global());
    assert!(!violations.iter().any(|v| format!("{v:?}").contains("I7NoModelPinning")), "{violations:?}");
}

#[tokio::test]
async fn model_swap_does_not_change_completion_authority() {
    // Completion is verifier-owned whichever backend produced the work.
    for backend in ["backend-a", "backend-b"] {
        let tmp = test_root();
        let (_, gate) = build_gate(tmp.path()).await;
        let gate = with_stub(gate, "accomplished", "allow");
        let dag_id = plan(&gate, vec![task("ms_a", None)]).await;
        gate.set_judge_policy(&dag_id, None, agency(), &user("eoj")).await.unwrap();
        let wih = gate.wih_pickup(&dag_id, "ms_a", backend).await.unwrap();
        let err = gate.wih_close_as(&wih, "DONE", &evidence(&ALL_EVIDENCE), None, Some(&agent(backend))).await.unwrap_err();
        assert_eq!(gate_err(&err).code, "completion_proposed", "{backend}");
    }
}
