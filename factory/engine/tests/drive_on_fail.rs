//! `on_fail` route-back in `drive`: the loop is bounded by `max_rounds`, then
//! the flow stops as degraded with a needs-you gate. Deterministic: nodes are
//! failed through the real Gate (pickup + FAILED close), no harness spawns.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::drive::hooks::NoHooks;
use allternit_factory_engine::drive::route::{ROUNDS_EXHAUSTED_REASON, ROUNDS_STATE};
use allternit_factory_engine::drive::{
    DriveExit, DriveOptions, DriveReport, Driver, ROUNDS_EXHAUSTED, ROUTE_BACK,
};
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::templates::{
    builtin_template, plan_from_template_with_roles, RoleMap, CLOSURE_DEGRADED_STATE, CLOSURE_STATE,
    EVIDENCE_PARAM,
};
use allternit_factory_engine::wait_gates::WaitGateKind;
use allternit_factory_engine::work::{project_dag, ready_nodes, DagState};
use allternit_factory_engine::{
    AllternitEvent, Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore, ReceiptStoreOptions,
};
use serde_json::Value;
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix("drive-on-fail-").tempdir_in(base).unwrap()
}

async fn build_gate(root: &Path) -> (Arc<Ledger>, Arc<Gate>) {
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

async fn dag(ledger: &Ledger, dag_id: &str) -> DagState {
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag_events: Vec<AllternitEvent> = events
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(Value::as_str) == Some(dag_id))
        .collect();
    project_dag(&dag_events, dag_id)
}

async fn count(ledger: &Ledger, ty: &str) -> usize {
    ledger
        .query(LedgerQuery { r#type: Some(ty.to_string()), ..Default::default() })
        .await
        .unwrap()
        .len()
}

async fn close(gate: &Gate, dag_id: &str, node_id: &str, status: &str, output: &str) {
    let wih = gate.wih_pickup(dag_id, node_id, "agent-x").await.unwrap();
    gate.wih_close_with(&wih, status, &[format!("test:{status}")], Some(output))
        .await
        .unwrap();
}

fn driver(root: &Path, ledger: &Arc<Ledger>, gate: &Arc<Gate>, dag_id: &str, dry_run: bool) -> Driver {
    Driver::new(
        root.to_path_buf(),
        ledger.clone(),
        Some(gate.clone()),
        DriveOptions { dag_id: dag_id.to_string(), dry_run, ..Default::default() },
        Arc::new(NoHooks),
    )
    .unwrap()
}

#[tokio::test]
async fn on_fail_routes_back_max_rounds_times_then_stops_degraded() {
    let tmp = test_root();
    let root = tmp.path();
    let (ledger, gate) = build_gate(root).await;

    let template = builtin_template("build-check-prove").unwrap();
    let mut roles = RoleMap::new();
    roles.insert("build".into(), "ao:claude".into());
    roles.insert("check".into(), "ao:codex".into());
    let params = HashMap::from([("intent".to_string(), "a login page".to_string())]);
    let plan = plan_from_template_with_roles(&gate, &template, &params, None, None, None, Some(&roles))
        .await
        .unwrap();
    let dag_id = plan.dag_id.clone();
    let (build, check, prove) = (&plan.nodes["build"], &plan.nodes["check"], &plan.nodes["prove"]);
    let max = template.effective_max_rounds().unwrap();
    assert_eq!(max, 3);

    // Minted nodes carry the labels drive reads; the root carries closure texts.
    let d = dag(&ledger, &dag_id).await;
    assert!(d.nodes[check].labels.contains(&format!("on_fail:{build}")));
    assert!(d.nodes[check].labels.contains(&"max_rounds:3".to_string()));
    assert_eq!(d.nodes[build].executor.as_deref(), Some("ao:claude"));
    assert_eq!(d.nodes[check].executor.as_deref(), Some("ao:codex"));
    let root_node = &d.nodes[&plan.root_node_id];
    assert!(root_node.state.contains_key(CLOSURE_DEGRADED_STATE));
    let prove_gate = &d.nodes[prove].wait_gates[0];
    assert_eq!(prove_gate.kind, WaitGateKind::Manual);
    assert_eq!(prove_gate.params[EVIDENCE_PARAM], "PROOF.md and proof/ files");
    assert!(prove_gate.description.contains("PROOF.md and proof/ files"));

    let mut routed = 0;
    for round in 1..=max + 1 {
        close(&gate, &dag_id, build, "DONE", "built it").await;
        close(&gate, &dag_id, check, "FAILED", &format!("check failed in round {round}")).await;
        assert_eq!(dag(&ledger, &dag_id).await.nodes[check].status, "FAILED");

        // Dry run: says what it would do, changes nothing.
        let mut dry = driver(root, &ledger, &gate, &dag_id, true);
        let report = dry.run(std::future::pending()).await.unwrap();
        assert_eq!(report.exit, Some(DriveExit::DryRun));
        let want = if round <= max {
            format!("would route back {check} → {build} (round {round}/{max}")
        } else {
            "rounds exhausted".to_string()
        };
        assert!(report.plan.iter().any(|l| l.contains(&want)), "{:?}", report.plan);
        assert_eq!(dag(&ledger, &dag_id).await.nodes[check].status, "FAILED");

        let live = driver(root, &ledger, &gate, &dag_id, true);
        let mut report = DriveReport::default();
        live.route_failures(&mut report).await.unwrap();
        let d = dag(&ledger, &dag_id).await;
        if round <= max {
            routed += 1;
            assert_eq!(report.routed_back, vec![(check.clone(), build.clone(), round)]);
            assert!(report.degraded.is_empty());
            assert_eq!(d.nodes[build].status, "READY", "target reopened");
            assert_eq!(d.nodes[check].status, "NEW", "failed node reopened");
            assert_eq!(d.nodes[check].state[ROUNDS_STATE], round.to_string());
            let desc = d.nodes[build].description.clone().unwrap_or_default();
            assert!(desc.contains(&format!("Round {round} of {max}")), "{desc}");
            assert!(desc.contains(&format!("{check}.out.md")), "failure output referenced: {desc}");
            assert_eq!(ready_nodes(&d), vec![build.clone()]);
        } else {
            assert!(report.routed_back.is_empty(), "no route back past max_rounds");
            assert_eq!(report.degraded, vec![check.clone()]);
            let root_node = &d.nodes[&plan.root_node_id];
            assert_eq!(root_node.state[CLOSURE_STATE], "degraded");
            assert_eq!(d.nodes[build].status, "DONE", "target not reopened");
            let g = d.nodes[check]
                .wait_gates
                .iter()
                .find(|g| g.params.get("reason").and_then(Value::as_str) == Some(ROUNDS_EXHAUSTED_REASON))
                .expect("needs-you gate");
            assert_eq!(g.kind, WaitGateKind::Manual);
            assert!(g.outcome.is_none());
            assert!(g.description.contains("rounds exhausted"), "{}", g.description);
            assert!(ready_nodes(&d).is_empty(), "nothing runs until a person decides");
        }
    }
    assert_eq!(routed, max);
    assert_eq!(count(&ledger, ROUTE_BACK).await, max as usize);
    assert_eq!(count(&ledger, ROUNDS_EXHAUSTED).await, 1);

    // A further pass does nothing: no loop.
    let live = driver(root, &ledger, &gate, &dag_id, true);
    let mut report = DriveReport::default();
    assert!(live.route_failures(&mut report).await.unwrap().is_empty());
    assert!(report.routed_back.is_empty() && report.degraded.is_empty());
    assert_eq!(count(&ledger, ROUTE_BACK).await, max as usize);
    assert_eq!(count(&ledger, ROUNDS_EXHAUSTED).await, 1);
}

#[tokio::test]
async fn nodes_between_target_and_failure_are_reopened() {
    let tmp = test_root();
    let root = tmp.path();
    let (ledger, gate) = build_gate(root).await;
    let template = allternit_factory_engine::templates::parse_markdown_template(
        "chain",
        r#"---
name: Chain
---
```yaml template-spec
max_rounds: 1
steps:
  - id: a
    title: A
  - id: b
    title: B
    blocked_by: [a]
  - id: c
    title: C
    blocked_by: [b]
    on_fail: a
```
"#,
    )
    .unwrap();
    let plan = plan_from_template_with_roles(&gate, &template, &HashMap::new(), None, None, None, None)
        .await
        .unwrap();
    let dag_id = plan.dag_id.clone();
    let (a, b, c) = (&plan.nodes["a"], &plan.nodes["b"], &plan.nodes["c"]);
    close(&gate, &dag_id, a, "DONE", "a").await;
    close(&gate, &dag_id, b, "DONE", "b").await;
    close(&gate, &dag_id, c, "FAILED", "c failed").await;

    let live = driver(root, &ledger, &gate, &dag_id, true);
    let mut report = DriveReport::default();
    live.route_failures(&mut report).await.unwrap();
    assert_eq!(report.routed_back, vec![(c.clone(), a.clone(), 1)]);
    let d = dag(&ledger, &dag_id).await;
    assert_eq!(d.nodes[a].status, "READY");
    assert_eq!(d.nodes[b].status, "NEW");
    assert_eq!(d.nodes[c].status, "NEW");

    // max_rounds 1: the next failure stops the flow.
    close(&gate, &dag_id, a, "DONE", "a").await;
    close(&gate, &dag_id, b, "DONE", "b").await;
    close(&gate, &dag_id, c, "FAILED", "c failed again").await;
    let mut report = DriveReport::default();
    live.route_failures(&mut report).await.unwrap();
    assert!(report.routed_back.is_empty());
    assert_eq!(report.degraded, vec![c.clone()]);
    let d = dag(&ledger, &dag_id).await;
    // No closure texts declared: drive still records the degraded closure.
    assert_eq!(d.nodes[&plan.root_node_id].state[CLOSURE_STATE], "degraded");
    assert_eq!(d.nodes[b].status, "DONE");
}
