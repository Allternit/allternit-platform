use std::path::PathBuf;
use std::sync::Arc;

use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::work::project_dag;
use allternit_factory_engine::{Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore, ReceiptStoreOptions};
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("allternit-factory-")
        .tempdir_in(base)
        .unwrap()
}

async fn build_gate(root: &PathBuf) -> (Arc<Ledger>, Gate) {
    let ledger = Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.clone()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }));

    let leases = Arc::new(
        Leases::new(LeasesOptions {
            root_dir: Some(root.clone()),
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
            root_dir: Some(root.clone()),
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
        root_dir: Some(root.clone()),
        actor_id: Some("gate".to_string()),
        strict_provenance: None,
        visual_provider: None,
        visual_config: None,
    });
    (ledger, gate)
}

async fn plan_two_children(gate: &Gate) -> (String, String, String, String) {
    let (_, dag_id, root_node) = gate.plan_new("Root", None).await.unwrap();
    let node_b = "n_b".to_string();
    let node_c = "n_c".to_string();
    gate.plan_refine(
        &dag_id,
        "add children",
        "agent",
        vec![
            Mutation::CreateNode {
                node_id: node_b.clone(),
                node_kind: "subtask".to_string(),
                title: "B".to_string(),
                parent_node_id: Some(root_node.clone()),
                execution_mode: "shared".to_string(),
                description: None,
                executor: None,
            },
            Mutation::CreateNode {
                node_id: node_c.clone(),
                node_kind: "subtask".to_string(),
                title: "C".to_string(),
                parent_node_id: Some(root_node.clone()),
                execution_mode: "shared".to_string(),
                description: None,
                executor: None,
            },
        ],
    )
    .await
    .unwrap();
    (dag_id, root_node, node_b, node_c)
}

fn children_of(dag: &allternit_factory_engine::work::DagState, parent: &str) -> Vec<String> {
    let mut children: Vec<String> = dag
        .nodes
        .values()
        .filter(|n| n.parent_node_id.as_deref() == Some(parent))
        .map(|n| n.node_id.clone())
        .collect();
    children.sort();
    children
}

#[tokio::test]
async fn reparent_node_moves_node_and_emits_event() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (dag_id, root_node, node_b, node_c) = plan_two_children(&gate).await;

    gate.plan_refine(
        &dag_id,
        "move c under b",
        "agent",
        vec![Mutation::ReparentNode {
            node_id: node_c.clone(),
            new_parent_id: Some(node_b.clone()),
        }],
    )
    .await
    .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let reparented = events
        .iter()
        .find(|e| e.r#type == "DagNodeReparented")
        .expect("DagNodeReparented event in ledger");
    assert_eq!(
        reparented.payload.get("dag_id").and_then(|v| v.as_str()),
        Some(dag_id.as_str())
    );
    assert_eq!(
        reparented.payload.get("node_id").and_then(|v| v.as_str()),
        Some(node_c.as_str())
    );
    assert_eq!(
        reparented
            .payload
            .get("new_parent_id")
            .and_then(|v| v.as_str()),
        Some(node_b.as_str())
    );
    assert_eq!(
        reparented
            .payload
            .get("old_parent_id")
            .and_then(|v| v.as_str()),
        Some(root_node.as_str())
    );

    let dag = project_dag(&events, &dag_id);
    assert_eq!(
        dag.nodes.get(&node_c).and_then(|n| n.parent_node_id.as_deref()),
        Some(node_b.as_str())
    );
    assert_eq!(children_of(&dag, &root_node), vec![node_b.clone()]);
    assert_eq!(children_of(&dag, &node_b), vec![node_c.clone()]);
}

#[tokio::test]
async fn reparent_node_to_root_via_none() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (dag_id, root_node, node_b, node_c) = plan_two_children(&gate).await;
    gate.mutate_with_decision(
        &dag_id,
        "move c under b",
        None,
        vec![Mutation::ReparentNode {
            node_id: node_c.clone(),
            new_parent_id: Some(node_b.clone()),
        }],
    )
    .await
    .unwrap();

    gate.mutate_with_decision(
        &dag_id,
        "move c back to root",
        None,
        vec![Mutation::ReparentNode {
            node_id: node_c.clone(),
            new_parent_id: None,
        }],
    )
    .await
    .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    // parent None = top level: n_c becomes a sibling of the dag root.
    assert_eq!(dag.nodes.get(&node_c).and_then(|n| n.parent_node_id.as_deref()), None);
    let mut top_level: Vec<String> = dag
        .nodes
        .values()
        .filter(|n| n.parent_node_id.is_none())
        .map(|n| n.node_id.clone())
        .collect();
    top_level.sort();
    assert_eq!(top_level, vec![root_node.clone(), node_c.clone()]);
    assert_eq!(children_of(&dag, &root_node), vec![node_b.clone()]);
}

#[tokio::test]
async fn reparent_under_own_descendant_is_rejected_as_cycle() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (dag_id, root_node, node_b, node_c) = plan_two_children(&gate).await;
    gate.plan_refine(
        &dag_id,
        "move c under b",
        "agent",
        vec![Mutation::ReparentNode {
            node_id: node_c.clone(),
            new_parent_id: Some(node_b.clone()),
        }],
    )
    .await
    .unwrap();

    // root -> b -> c; reparenting root under c would close the loop.
    let err = gate
        .mutate_with_decision(
            &dag_id,
            "attempt cycle",
            None,
            vec![Mutation::ReparentNode {
                node_id: root_node.clone(),
                new_parent_id: Some(node_c.clone()),
            }],
        )
        .await
        .expect_err("cycle reparent must fail");
    assert!(
        err.to_string().contains("cycle"),
        "error should mention cycle, got: {}",
        err
    );

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    assert_eq!(dag.nodes.get(&root_node).and_then(|n| n.parent_node_id.as_deref()), None);
}

#[tokio::test]
async fn reparent_under_nonexistent_parent_is_rejected() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (dag_id, _, node_b, _) = plan_two_children(&gate).await;

    let err = gate
        .plan_refine(
            &dag_id,
            "attempt missing parent",
            "agent",
            vec![Mutation::ReparentNode {
                node_id: node_b.clone(),
                new_parent_id: Some("n_missing".to_string()),
            }],
        )
        .await
        .expect_err("reparent under missing parent must fail");
    assert!(
        err.to_string().contains("not found"),
        "error should mention not found, got: {}",
        err
    );

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    assert!(dag.nodes.contains_key(&node_b));
}
