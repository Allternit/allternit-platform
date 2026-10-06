//! Review #18: lifecycle validation at real mutation/CLI/projection boundaries.
use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::work::project_dag;
use allternit_factory_engine::{
    Actor, ActorType, AllternitEvent, Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore,
    ReceiptStoreOptions,
};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

fn change(node: &str, from: &str, to: &str) -> Mutation {
    Mutation::ChangeStatus {
        node_id: node.into(),
        from: from.into(),
        to: to.into(),
        reason: None,
    }
}
async fn status(ledger: &Ledger, dag: &str, node: &str) -> String {
    project_dag(&ledger.query(LedgerQuery::default()).await.unwrap(), dag).nodes[node]
        .status
        .clone()
}
async fn count(ledger: &Ledger) -> usize {
    ledger.query(LedgerQuery::default()).await.unwrap().len()
}
#[tokio::test]
async fn status_mutation_rejects_failed_to_done_and_unknown_without_appending() {
    let root = tempfile::tempdir().unwrap();
    let (ledger, gate) = build_gate(root.path()).await;
    let (_, dag, node) = gate.plan_new("review", None).await.unwrap();
    gate.mutate_with_decision(
        &dag,
        "start",
        None,
        vec![change(&node, "READY", "IN_PROGRESS")],
    )
    .await
    .unwrap();
    gate.mutate_with_decision(
        &dag,
        "fail",
        None,
        vec![change(&node, "IN_PROGRESS", "FAILED")],
    )
    .await
    .unwrap();
    for to in ["DONE", "WEIRD"] {
        let before = count(&ledger).await;
        assert!(
            gate.mutate_with_decision(&dag, "bad status", None, vec![change(&node, "FAILED", to)])
                .await
                .is_err(),
            "FAILED -> {to}"
        );
        assert_eq!(count(&ledger).await, before);
        assert_eq!(status(&ledger, &dag, &node).await, "FAILED");
    }
}
#[tokio::test]
async fn status_mutation_checks_current_source_and_batch_order() {
    let root = tempfile::tempdir().unwrap();
    let (ledger, gate) = build_gate(root.path()).await;
    let (_, dag, node) = gate.plan_new("review", None).await.unwrap();
    // Prime the scope's policy injection receipt; rejected mutations must
    // append neither status changes nor decision/prompt mutation records.
    gate.mutate_with_decision(&dag, "no-op", None, vec![change(&node, "READY", "READY")])
        .await
        .unwrap();
    let before = count(&ledger).await;
    assert!(gate
        .plan_refine(
            &dag,
            "stale",
            "test",
            vec![change(&node, "NEW", "IN_PROGRESS")]
        )
        .await
        .is_err());
    assert_eq!(count(&ledger).await, before);
    assert!(gate
        .mutate_with_decision(
            &dag,
            "bad batch",
            None,
            vec![
                change(&node, "READY", "IN_PROGRESS"),
                change(&node, "IN_PROGRESS", "WEIRD")
            ]
        )
        .await
        .is_err());
    assert_eq!(count(&ledger).await, before);
    assert!(gate
        .mutate_with_decision(&dag, "missing", None, vec![change("ghost", "NEW", "READY")])
        .await
        .is_err());
    gate.plan_refine(
        &dag,
        "valid batch",
        "test",
        vec![
            change(&node, "READY", "IN_PROGRESS"),
            change(&node, "IN_PROGRESS", "FAILED"),
        ],
    )
    .await
    .unwrap();
    assert_eq!(status(&ledger, &dag, &node).await, "FAILED");
    gate.mutate_with_decision(&dag, "reopen", None, vec![change(&node, "FAILED", "NEW")])
        .await
        .unwrap();
    assert_eq!(status(&ledger, &dag, &node).await, "READY");
}
fn event(ty: &str, payload: serde_json::Value) -> AllternitEvent {
    AllternitEvent {
        event_id: "e".into(),
        ts: "2026-09-30T12:00:00Z".into(),
        actor: Actor {
            r#type: ActorType::Agent,
            id: "test".into(),
        },
        scope: None,
        r#type: ty.into(),
        payload,
        provenance: None,
    }
}
#[test]
fn status_projection_rejects_illegal_and_unknown_ledger_changes() {
    let mut events = vec![
        event(
            "DagNodeCreated",
            json!({"dag_id":"d", "node_id":"n", "title":"review"}),
        ),
        event(
            "DagNodeStatusChanged",
            json!({"dag_id":"d", "node_id":"n", "to":"IN_PROGRESS"}),
        ),
        event(
            "DagNodeStatusChanged",
            json!({"dag_id":"d", "node_id":"n", "to":"FAILED"}),
        ),
    ];
    for to in ["DONE", "WEIRD"] {
        let mut bad = events.clone();
        bad.push(event(
            "DagNodeStatusChanged",
            json!({"dag_id":"d", "node_id":"n", "from":"NEW", "to":to}),
        ));
        assert_eq!(project_dag(&bad, "d").nodes["n"].status, "FAILED");
    }
    events.push(event(
        "DagNodeStatusChanged",
        json!({"dag_id":"other", "node_id":"n", "to":"NEW"}),
    ));
    assert_eq!(project_dag(&events, "d").nodes["n"].status, "FAILED");
}
#[tokio::test]
async fn status_cli_reads_current_source_and_rejects_weird() {
    let root = tempfile::tempdir().unwrap();
    let (ledger, gate) = build_gate(root.path()).await;
    let (_, dag, node) = gate.plan_new("review", None).await.unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_allternit-factory"))
            .args(["internal", "core"])
            .arg("--root")
            .arg(root.path())
            .arg("work")
            .args(args)
            .output()
            .unwrap()
    };
    assert!(
        run(&["update", &dag, "--node", &node, "--status", "IN_PROGRESS"])
            .status
            .success()
    );
    assert!(
        run(&["update", &dag, "--node", &node, "--status", "FAILED"])
            .status
            .success()
    );
    assert_eq!(status(&ledger, &dag, &node).await, "FAILED");
    let before = count(&ledger).await;
    assert!(!run(&[
        "update",
        &dag,
        "--node",
        &node,
        "--status",
        "WEIRD",
        "--title",
        "must not update"
    ])
    .status
    .success());
    assert!(!run(&["close", &dag]).status.success());
    assert_eq!(count(&ledger).await, before);
    assert!(run(&["reopen", &dag]).status.success());
    assert_eq!(status(&ledger, &dag, &node).await, "READY");
}
