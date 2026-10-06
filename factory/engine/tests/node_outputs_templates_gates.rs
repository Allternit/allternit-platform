//! Node outputs + `{{ node.output }}` templating, templates -> WIH DAG, and
//! node-scoped wait-gates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::fence::Fence;
use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::templates::{parse_markdown_template, plan_from_template};
use allternit_factory_engine::wait_gates::{GateOutcome, WaitGateKind};
use allternit_factory_engine::work::needs_you::pending_manual_gates;
use allternit_factory_engine::work::{project_dag, ready_nodes, DagState};
use allternit_factory_engine::{
    Actor, ActorType, Gate, GateError, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore,
    ReceiptStoreOptions, WihPickupOptions, AllternitEvent,
};
use serde_json::{json, Value};
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("allternit-rails-")
        .tempdir_in(base)
        .unwrap()
}

async fn build_gate(root: &Path) -> (Arc<Ledger>, Gate) {
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
            auto_renewal_enabled: true,
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
    let gate = Gate::new(GateOptions {
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
    });
    (ledger, gate)
}

fn node(id: &str, parent: &str, description: Option<&str>) -> Mutation {
    Mutation::CreateNode {
        node_id: id.to_string(),
        node_kind: "task".to_string(),
        title: id.to_uppercase(),
        parent_node_id: Some(parent.to_string()),
        execution_mode: "shared".to_string(),
        description: description.map(|s| s.to_string()),
        executor: None,
    }
}

fn blocked_by(blocker: &str, blocked: &str) -> Mutation {
    Mutation::AddBlockedBy {
        from_node_id: blocker.to_string(),
        to_node_id: blocked.to_string(),
    }
}

fn gate_err(err: &anyhow::Error) -> &GateError {
    GateError::from_anyhow(err).unwrap_or_else(|| panic!("expected GateError, got {err:#}"))
}

async fn dag(ledger: &Ledger, dag_id: &str) -> DagState {
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag_events: Vec<AllternitEvent> = events
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .collect();
    project_dag(&dag_events, dag_id)
}

/// Unique node ids per test: node ids are global in the active-WIH check.
fn ids(tag: &str) -> (String, String, String) {
    (format!("{tag}_a"), format!("{tag}_b"), format!("{tag}_c"))
}

/// a -> b (b blocked by a); c unrelated. Returns (dag_id, root).
async fn plan_abc(gate: &Gate, a: &str, b: &str, c: &str, b_desc: Option<&str>, c_desc: Option<&str>) -> (String, String) {
    let (_, dag_id, root) = gate.plan_new("outputs", None).await.unwrap();
    gate.plan_refine(
        &dag_id,
        "a feeds b",
        "agent",
        vec![
            node(a, &root, None),
            node(b, &root, b_desc),
            node(c, &root, c_desc),
            blocked_by(a, b),
        ],
    )
    .await
    .unwrap();
    (dag_id, root)
}

async fn run_to_done(gate: &Gate, dag_id: &str, node_id: &str, output: Option<&str>) -> Option<String> {
    let wih = gate.wih_pickup(dag_id, node_id, "agent-x").await.unwrap();
    gate.wih_close_with(&wih, "DONE", &["ok".to_string()], output)
        .await
        .unwrap()
}

#[tokio::test]
async fn refine_rejects_placeholder_to_unknown_node_without_emitting() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let (prompt_id, dag_id, root_node) = gate.plan_new("validate", None).await.unwrap();
    let work_events = |events: Vec<AllternitEvent>| {
        events
            .into_iter()
            .filter(|e| e.r#type.starts_with("Dag") || e.r#type.starts_with("Prompt"))
            .count()
    };
    let before = work_events(ledger.query(LedgerQuery::default()).await.unwrap());

    let err = gate
        .plan_refine(
            &dag_id,
            "bad ref",
            "agent",
            vec![node("v_x", &root_node, Some("use {{ ghost.output }}"))],
        )
        .await
        .unwrap_err();
    let g = gate_err(&err);
    assert_eq!(g.gate, "gate0.plan");
    assert_eq!(g.code, "template_ref_unknown_node");
    assert_eq!(g.details["ref_node_id"], json!("ghost"));
    assert_eq!(g.details["provenance"]["prompt_id"], json!(prompt_id));
    assert!(g.details["provenance"]["delta_id"].is_string());
    // Nothing about the rejected batch reached the ledger (a policy-scope
    // injection event may, which is not a work mutation).
    assert_eq!(work_events(ledger.query(LedgerQuery::default()).await.unwrap()), before);

    // Same-batch forward references are fine; so is the agent-decision path.
    gate.plan_refine(
        &dag_id,
        "ok ref",
        "agent",
        vec![
            node("v_y", &root_node, Some("{{ v_z.output_path }}")),
            node("v_z", &root_node, None),
        ],
    )
    .await
    .unwrap();
    let err = gate
        .mutate_with_decision(
            &dag_id,
            "decision",
            None,
            vec![Mutation::UpdateNode {
                node_id: "v_y".to_string(),
                patch: json!({ "description": "{{ nope.output }}" }),
            }],
        )
        .await
        .unwrap_err();
    assert_eq!(gate_err(&err).code, "template_ref_unknown_node");
}

#[tokio::test]
async fn refine_rejects_bad_executor_and_stores_good_one() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let (_, dag_id, root_node) = gate.plan_new("executor", None).await.unwrap();
    let mut bad = node("ex_a", &root_node, None);
    if let Mutation::CreateNode { executor, .. } = &mut bad {
        *executor = Some("human".to_string());
    }
    let err = gate.plan_refine(&dag_id, "bad", "agent", vec![bad]).await.unwrap_err();
    assert_eq!(gate_err(&err).code, "invalid_executor");

    let mut good = node("ex_b", &root_node, None);
    if let Mutation::CreateNode { executor, .. } = &mut good {
        *executor = Some("bot:editor".to_string());
    }
    gate.plan_refine(&dag_id, "good", "agent", vec![good]).await.unwrap();
    gate.plan_refine(
        &dag_id,
        "patch",
        "agent",
        vec![Mutation::UpdateNode {
            node_id: "ex_b".to_string(),
            patch: json!({ "executor": "ao:kimi" }),
        }],
    )
    .await
    .unwrap();
    let d = dag(&ledger, &dag_id).await;
    // UpdateNode events now carry dag_id, so the dag-scoped projection sees them.
    assert_eq!(d.nodes["ex_b"].executor.as_deref(), Some("ao:kimi"));
}

#[tokio::test]
async fn close_with_output_records_blob_receipt_view_and_event() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("out");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;

    let receipt_id = run_to_done(&gate, &dag_id, &a, Some("# capture\nclip-01 at 00:12"))
        .await
        .expect("output receipt");
    let d = dag(&ledger, &dag_id).await;
    let out = d.nodes[&a].output.clone().expect("output ref on node");
    assert_eq!(out.receipt_id, receipt_id);
    assert_eq!(out.output_path, format!(".allternit/work/dags/{dag_id}/nodes/{a}.out.md"));
    assert_eq!(
        std::fs::read_to_string(root.join(&out.output_path)).unwrap(),
        "# capture\nclip-01 at 00:12"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".allternit/blobs").join(&out.blob_id)).unwrap(),
        "# capture\nclip-01 at 00:12"
    );
    assert!(root
        .join(".allternit/receipts")
        .join(&receipt_id)
        .join("receipt.json")
        .exists());
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let recorded = events
        .iter()
        .find(|e| e.r#type == "DagNodeOutputRecorded")
        .expect("DagNodeOutputRecorded");
    assert_eq!(recorded.payload["node_id"], json!(a));
    let close = events
        .iter()
        .find(|e| e.r#type == "WIHCloseRequested")
        .unwrap();
    assert!(close.payload["evidence_refs"]
        .as_array()
        .unwrap()
        .contains(&json!(format!("receipt:{receipt_id}"))));
    assert_eq!(d.nodes[&a].status, "DONE");
    assert_eq!(d.nodes[&b].status, "READY");
}

#[tokio::test]
async fn close_requires_evidence_or_output() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (_ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("ev");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;
    let wih = gate.wih_pickup(&dag_id, &a, "agent").await.unwrap();
    assert!(gate.wih_close_with(&wih, "DONE", &[], None).await.is_err());
    // Output alone is evidence.
    assert!(gate
        .wih_close_with(&wih, "DONE", &[], Some("result"))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn pickup_resolves_output_placeholders_into_wih_context() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("res");
    let desc = format!("Cut from:\n{{{{ {a}.output }}}}\nfile: {{{{ {a}.output_path }}}}");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, Some(&desc), None).await;
    run_to_done(&gate, &dag_id, &a, Some("CAPTURE-NOTES")).await;

    let pickup = gate
        .wih_pickup_detailed(&dag_id, &b, "agent", WihPickupOptions { role: None, fresh: true })
        .await
        .unwrap();
    let text = pickup.resolved_description.clone().expect("resolved text");
    let abs = root.join(format!(".allternit/work/dags/{dag_id}/nodes/{a}.out.md"));
    // S7: the inlined output is nonce-fenced and the rule is stated once.
    let fence = Fence::with_nonce(pickup.fence_nonce.clone());
    assert_eq!(
        text,
        format!(
            "{}\n\nCut from:\n{}\nfile: {}",
            fence.instruction(),
            fence.wrap(&format!("node:{a}"), "CAPTURE-NOTES"),
            abs.display()
        )
    );
    let prompt_path = pickup.resolved_prompt_path.clone().unwrap();
    assert_eq!(std::fs::read_to_string(&prompt_path).unwrap(), text);

    // The DAG itself is not rewritten.
    let d = dag(&ledger, &dag_id).await;
    assert_eq!(d.nodes[&b].description.as_deref(), Some(desc.as_str()));

    // ContextPack: predecessor output ref + inline text + resolved description.
    let pack: Value =
        serde_json::from_str(&std::fs::read_to_string(pickup.context_pack_path.unwrap()).unwrap())
            .unwrap();
    let outputs = pack["dependency_outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0]["node_id"], json!(a));
    assert_eq!(outputs[0]["text"], json!(fence.wrap(&format!("node:{a}"), "CAPTURE-NOTES")));
    assert_eq!(pack["untrusted_fence"]["nonce"], json!(pickup.fence_nonce));
    assert_eq!(outputs[0]["truncated"], json!(false));
    assert_eq!(pack["resolved_description"], json!(text));

    // WIH projection exposes the resolved prompt path; WIHCreated records refs.
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let created = events
        .iter()
        .find(|e| e.r#type == "WIHCreated" && e.payload["wih_id"] == json!(pickup.wih_id))
        .unwrap();
    assert_eq!(created.payload["template_refs"].as_array().unwrap().len(), 2);
    assert_eq!(created.payload["resolved_prompt_path"], json!(prompt_path));
}

#[tokio::test]
async fn context_pack_truncates_large_outputs() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (_ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("big");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;
    let big = "é".repeat(allternit_factory_engine::gate::CONTEXT_PACK_OUTPUT_INLINE_CAP);
    run_to_done(&gate, &dag_id, &a, Some(&big)).await;
    let pickup = gate
        .wih_pickup_detailed(&dag_id, &b, "agent", WihPickupOptions { role: None, fresh: true })
        .await
        .unwrap();
    let pack: Value =
        serde_json::from_str(&std::fs::read_to_string(pickup.context_pack_path.unwrap()).unwrap())
            .unwrap();
    let out = &pack["dependency_outputs"][0];
    assert_eq!(out["truncated"], json!(true));
    // The cap applies to the output; the fence markers wrap the capped text.
    let fence = Fence::with_nonce(pickup.fence_nonce.clone());
    let overhead = fence.wrap(&format!("node:{a}"), "").len();
    assert!(
        out["text"].as_str().unwrap().len()
            <= allternit_factory_engine::gate::CONTEXT_PACK_OUTPUT_INLINE_CAP + overhead
    );
    assert_eq!(out["size_bytes"], json!(big.len()));
}

#[tokio::test]
async fn pickup_refuses_ref_to_non_predecessor() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (_ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("np");
    // c references a, but c is not blocked_by a.
    let c_desc = format!("{{{{ {a}.output }}}}");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, Some(&c_desc)).await;
    run_to_done(&gate, &dag_id, &a, Some("x")).await;
    let err = gate.wih_pickup(&dag_id, &c, "agent").await.unwrap_err();
    let g = gate_err(&err);
    assert_eq!(g.gate, "gate1.pickup");
    assert_eq!(g.code, "template_ref_not_predecessor");
    assert_eq!(g.node_id.as_deref(), Some(c.as_str()));
}

#[tokio::test]
async fn pickup_refuses_ref_to_predecessor_without_output() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (_ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("nout");
    let b_desc = format!("{{{{ {a}.output }}}}");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, Some(&b_desc), None).await;
    run_to_done(&gate, &dag_id, &a, None).await;
    let err = gate.wih_pickup(&dag_id, &b, "agent").await.unwrap_err();
    assert_eq!(gate_err(&err).code, "template_ref_output_missing");
}

#[tokio::test]
async fn transitive_predecessor_ref_is_allowed() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (_ledger, gate) = build_gate(&root).await;
    let (_, dag_id, root_node) = gate.plan_new("chain", None).await.unwrap();
    gate.plan_refine(
        &dag_id,
        "chain",
        "agent",
        vec![
            node("tr_a", &root_node, None),
            node("tr_b", &root_node, None),
            node("tr_c", &root_node, Some("from a: {{ tr_a.output }}")),
            blocked_by("tr_a", "tr_b"),
            blocked_by("tr_b", "tr_c"),
        ],
    )
    .await
    .unwrap();
    run_to_done(&gate, &dag_id, "tr_a", Some("A-OUT")).await;
    run_to_done(&gate, &dag_id, "tr_b", None).await;
    let pickup = gate
        .wih_pickup_detailed(&dag_id, "tr_c", "agent", WihPickupOptions::default())
        .await
        .unwrap();
    let fence = Fence::with_nonce(pickup.fence_nonce.clone());
    assert_eq!(
        pickup.resolved_description,
        Some(format!("{}\n\nfrom a: {}", fence.instruction(), fence.wrap("node:tr_a", "A-OUT")))
    );
}

#[tokio::test]
async fn pickup_refuses_node_with_unmet_blockers() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (_ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("blk");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;
    let err = gate.wih_pickup(&dag_id, &b, "agent").await.unwrap_err();
    let g = gate_err(&err);
    assert_eq!(g.code, "blocked_by_unmet");
    assert_eq!(g.details["blocked_by"], json!([a]));
}

#[tokio::test]
async fn manual_gate_blocks_until_resolved_by_explicit_actor() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("man");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;
    let gate_id = gate
        .add_node_wait_gate(&dag_id, &c, WaitGateKind::Manual, Some("Eoj reviews".into()), HashMap::new(), "test")
        .await
        .unwrap();

    let d = dag(&ledger, &dag_id).await;
    assert_eq!(d.nodes[&c].status, "NEW");
    let ready = ready_nodes(&d);
    assert!(ready.contains(&a));
    assert!(!ready.contains(&c), "gated node must not be ready");
    let err = gate.wih_pickup(&dag_id, &c, "agent").await.unwrap_err();
    assert_eq!(gate_err(&err).code, "wait_gate_unresolved");

    // Needs-you projection lists it (upstream done: c has no blockers).
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let pending = pending_manual_gates(&events);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].gate_id, gate_id);
    assert!(pending[0].deps_done);

    // Manual resolve needs an explicit, non-gate actor.
    let err = gate
        .resolve_node_wait_gate(&dag_id, &c, &gate_id, GateOutcome::Ok, None, None)
        .await
        .unwrap_err();
    assert_eq!(gate_err(&err).code, "manual_resolve_requires_actor");
    let gate_actor = Actor { r#type: ActorType::Gate, id: "gate".into() };
    assert!(gate
        .resolve_node_wait_gate(&dag_id, &c, &gate_id, GateOutcome::Ok, Some(gate_actor), None)
        .await
        .is_err());

    let eoj = Actor { r#type: ActorType::User, id: "eoj".into() };
    gate.resolve_node_wait_gate(&dag_id, &c, &gate_id, GateOutcome::Ok, Some(eoj), Some("looks good".into()))
        .await
        .unwrap();
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let resolved = events
        .iter()
        .find(|e| e.r#type == "DagNodeWaitGateResolved")
        .unwrap();
    assert_eq!(resolved.actor.id, "eoj");
    assert_eq!(resolved.payload["resolved_by"], json!("user:eoj"));
    assert!(pending_manual_gates(&events).is_empty());

    let d = dag(&ledger, &dag_id).await;
    assert_eq!(d.nodes[&c].status, "READY");
    assert!(ready_nodes(&d).contains(&c));
    gate.wih_pickup(&dag_id, &c, "agent").await.unwrap();
}

#[tokio::test]
async fn failed_manual_gate_keeps_blocking_and_can_be_re_resolved() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("fail");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;
    let gid = gate
        .add_node_wait_gate(&dag_id, &c, WaitGateKind::Manual, None, HashMap::new(), "test")
        .await
        .unwrap();
    let eoj = || Some(Actor { r#type: ActorType::User, id: "eoj".into() });
    gate.resolve_node_wait_gate(&dag_id, &c, &gid, GateOutcome::Failed, eoj(), None)
        .await
        .unwrap();
    assert!(!ready_nodes(&dag(&ledger, &dag_id).await).contains(&c));
    gate.resolve_node_wait_gate(&dag_id, &c, &gid, GateOutcome::Ok, eoj(), None)
        .await
        .unwrap();
    assert!(ready_nodes(&dag(&ledger, &dag_id).await).contains(&c));
    let err = gate
        .resolve_node_wait_gate(&dag_id, &c, &gid, GateOutcome::Ok, eoj(), None)
        .await
        .unwrap_err();
    assert_eq!(gate_err(&err).code, "already_resolved");
}

#[tokio::test]
async fn timer_gate_resolves_lazily_on_readiness_check() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("tmr");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;
    let mut past = HashMap::new();
    past.insert("until".to_string(), json!("2020-01-01T00:00:00Z"));
    let mut future = HashMap::new();
    future.insert("until".to_string(), json!("2999-01-01T00:00:00Z"));
    gate.add_node_wait_gate(&dag_id, &c, WaitGateKind::Timer, None, past, "test")
        .await
        .unwrap();
    gate.add_node_wait_gate(&dag_id, &a, WaitGateKind::Timer, None, future, "test")
        .await
        .unwrap();

    let d = dag(&ledger, &dag_id).await;
    // Elapsed timer: ready by the clock even before a resolution event exists.
    let ready = ready_nodes(&d);
    assert!(ready.contains(&c));
    assert!(!ready.contains(&a));

    // Pickup is a readiness check: it records the timer resolution.
    gate.wih_pickup(&dag_id, &c, "agent").await.unwrap();
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let resolved: Vec<_> = events
        .iter()
        .filter(|e| e.r#type == "DagNodeWaitGateResolved")
        .collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].payload["node_id"], json!(c));
    assert_eq!(resolved[0].payload["reason"], json!("timer elapsed"));

    let err = gate.wih_pickup(&dag_id, &a, "agent").await.unwrap_err();
    assert_eq!(gate_err(&err).code, "wait_gate_unresolved");
}

#[tokio::test]
async fn wait_gate_validation() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (_ledger, gate) = build_gate(&root).await;
    let (a, b, c) = ids("wgv");
    let (dag_id, _) = plan_abc(&gate, &a, &b, &c, None, None).await;
    let err = gate
        .add_node_wait_gate(&dag_id, "missing", WaitGateKind::Manual, None, HashMap::new(), "t")
        .await
        .unwrap_err();
    assert_eq!(gate_err(&err).code, "wait_gate_unknown_node");
    let err = gate
        .add_node_wait_gate(&dag_id, &a, WaitGateKind::Timer, None, HashMap::new(), "t")
        .await
        .unwrap_err();
    assert_eq!(gate_err(&err).code, "wait_gate_invalid_params");
    run_to_done(&gate, &dag_id, &a, None).await;
    let err = gate
        .add_node_wait_gate(&dag_id, &a, WaitGateKind::Manual, None, HashMap::new(), "t")
        .await
        .unwrap_err();
    assert_eq!(gate_err(&err).code, "wait_gate_terminal_node");
}

const PROMO: &str = r#"---
name: Motion promo
description: capture -> cut -> gate-2 review -> re-record
---

```yaml template-spec
params:
  - name: topic
steps:
  - id: capture
    title: "Capture {{ params.topic }}"
    description: "Screen-record the {{ params.topic }} flow"
    executor: "ao:claude"
  - id: cut
    title: Cut
    description: "Cut the promo from these capture notes:\n{{ capture.output }}"
    blocked_by: [capture]
  - id: review
    title: Gate-2 review
    blocked_by: [cut]
    wait_gate:
      kind: manual
      description: "Eoj reviews the {{ params.topic }} cut"
  - id: rerecord
    title: Re-record
    description: "Re-record using notes at {{ capture.output_path }}"
    blocked_by: [review]
```
"#;

#[tokio::test]
async fn template_instantiates_wih_dag_with_params_edges_gates_and_provenance() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;
    let template = parse_markdown_template("promo", PROMO).unwrap();

    // Missing required param: rejected before any plan exists.
    let before = ledger.query(LedgerQuery::default()).await.unwrap().len();
    assert!(plan_from_template(&gate, &template, &HashMap::new(), None, None)
        .await
        .is_err());
    assert_eq!(ledger.query(LedgerQuery::default()).await.unwrap().len(), before);

    let mut params = HashMap::new();
    params.insert("topic".to_string(), "Projects".to_string());
    let result = plan_from_template(&gate, &template, &params, None, None)
        .await
        .unwrap();
    assert_eq!(result.nodes.len(), 4);
    let capture = &result.nodes["capture"];
    let cut = &result.nodes["cut"];
    let review = &result.nodes["review"];
    let rerecord = &result.nodes["rerecord"];

    let d = dag(&ledger, &result.dag_id).await;
    assert_eq!(d.nodes.len(), 5, "root + 4 steps");
    assert_eq!(d.nodes[capture].title, "Capture Projects");
    assert_eq!(d.nodes[capture].executor.as_deref(), Some("ao:claude"));
    assert_eq!(
        d.nodes[cut].description.as_deref(),
        Some(format!("Cut the promo from these capture notes:\n{{{{ {capture}.output }}}}").as_str())
    );
    assert_eq!(d.nodes[capture].parent_node_id.as_deref(), Some(result.root_node_id.as_str()));
    let edges: Vec<(String, String)> = d
        .edges
        .iter()
        .map(|e| (e.from_node_id.clone(), e.to_node_id.clone()))
        .collect();
    assert!(edges.contains(&(capture.clone(), cut.clone())));
    assert!(edges.contains(&(cut.clone(), review.clone())));
    assert!(edges.contains(&(review.clone(), rerecord.clone())));
    assert_eq!(d.nodes[review].wait_gates.len(), 1);
    assert_eq!(d.nodes[review].wait_gates[0].description, "Eoj reviews the Projects cut");

    // Every template mutation carries the refine delta's provenance.
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    for e in events.iter().filter(|e| {
        matches!(
            e.r#type.as_str(),
            "DagNodeCreated" | "DagEdgeAdded" | "DagNodeWaitGateAdded"
        ) && e.payload["dag_id"] == json!(result.dag_id)
            && e.payload["node_id"] != json!(result.root_node_id)
    }) {
        let prov = e.provenance.as_ref().expect("provenance");
        assert_eq!(prov.delta_id.as_deref(), Some(result.delta_id.as_str()), "{}", e.r#type);
    }

    // Only capture is ready: the plan root is blocked by the terminal step.
    assert_eq!(ready_nodes(&d), vec![capture.clone()]);
    assert!(edges.contains(&(rerecord.clone(), result.root_node_id.clone())));

    // Flow: capture -> cut resolves output -> review gated -> manual resolve.
    run_to_done(&gate, &result.dag_id, capture, Some("clip-01 00:12 hero shot")).await;
    let pickup = gate
        .wih_pickup_detailed(&result.dag_id, cut, "agent", WihPickupOptions::default())
        .await
        .unwrap();
    let fence = Fence::with_nonce(pickup.fence_nonce.clone());
    assert_eq!(
        pickup.resolved_description,
        Some(format!(
            "{}\n\nCut the promo from these capture notes:\n{}",
            fence.instruction(),
            fence.wrap(&format!("node:{capture}"), "clip-01 00:12 hero shot")
        ))
    );
    gate.wih_close_with(&pickup.wih_id, "DONE", &[], Some("cut v1"))
        .await
        .unwrap();
    let d = dag(&ledger, &result.dag_id).await;
    assert!(!ready_nodes(&d).contains(review));
    let err = gate.wih_pickup(&result.dag_id, review, "agent").await.unwrap_err();
    assert_eq!(gate_err(&err).code, "wait_gate_unresolved");
    let gid = d.nodes[review].wait_gates[0].gate_id.clone();
    gate.resolve_node_wait_gate(
        &result.dag_id,
        review,
        &gid,
        GateOutcome::Ok,
        Some(Actor { r#type: ActorType::User, id: "eoj".into() }),
        None,
    )
    .await
    .unwrap();
    assert!(ready_nodes(&dag(&ledger, &result.dag_id).await).contains(review));
}
