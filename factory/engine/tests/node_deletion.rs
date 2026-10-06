use std::path::PathBuf;
use std::sync::Arc;

use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::work::{project_dag, ready_nodes};
use allternit_factory_engine::{Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore, ReceiptStoreOptions};
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("allternit-rails-")
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

#[tokio::test]
async fn delete_node_removes_it_from_projection_and_emits_event() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (_, dag_id, root_node) = gate.plan_new("Root", None).await.unwrap();
    let node_b = "n_b".to_string();

    gate.plan_refine(
        &dag_id,
        "add child",
        "agent",
        vec![Mutation::CreateNode {
            node_id: node_b.clone(),
            node_kind: "subtask".to_string(),
            title: "Child B".to_string(),
            parent_node_id: Some(root_node.clone()),
            execution_mode: "shared".to_string(),
            description: None,
            executor: None,
        }],
    )
    .await
    .unwrap();

    // Rename via UpdateNode first so the removal event can carry the final title.
    gate.mutate_with_decision(
        &dag_id,
        "rename child",
        None,
        vec![Mutation::UpdateNode {
            node_id: node_b.clone(),
            patch: serde_json::json!({ "title": "Renamed B" }),
        }],
    )
    .await
    .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    assert_eq!(
        dag.nodes.get(&node_b).map(|n| n.title.as_str()),
        Some("Renamed B")
    );
    let children: Vec<&String> = dag
        .nodes
        .values()
        .filter(|n| n.parent_node_id.as_deref() == Some(root_node.as_str()))
        .map(|n| &n.node_id)
        .collect();
    assert_eq!(children, vec![&node_b]);

    gate.mutate_with_decision(
        &dag_id,
        "delete child",
        None,
        vec![Mutation::DeleteNode {
            node_id: node_b.clone(),
        }],
    )
    .await
    .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();

    let removed = events
        .iter()
        .find(|e| e.r#type == "DagNodeRemoved")
        .expect("DagNodeRemoved event in ledger");
    assert_eq!(
        removed.payload.get("dag_id").and_then(|v| v.as_str()),
        Some(dag_id.as_str())
    );
    assert_eq!(
        removed.payload.get("node_id").and_then(|v| v.as_str()),
        Some(node_b.as_str())
    );
    assert_eq!(
        removed.payload.get("title").and_then(|v| v.as_str()),
        Some("Renamed B")
    );
    assert_eq!(
        removed
            .payload
            .get("parent_node_id")
            .and_then(|v| v.as_str()),
        Some(root_node.as_str())
    );

    let dag = project_dag(&events, &dag_id);
    assert!(!dag.nodes.contains_key(&node_b));
    assert!(dag.nodes.contains_key(&root_node));
    assert!(!ready_nodes(&dag).contains(&node_b));
    let children: Vec<&String> = dag
        .nodes
        .values()
        .filter(|n| n.parent_node_id.as_deref() == Some(root_node.as_str()))
        .map(|n| &n.node_id)
        .collect();
    assert!(children.is_empty());
}

#[tokio::test]
async fn delete_node_drops_edges_touching_the_node() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

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

    gate.mutate_with_decision(
        &dag_id,
        "c blocked by b",
        None,
        vec![Mutation::AddBlockedBy {
            from_node_id: node_b.clone(),
            to_node_id: node_c.clone(),
        }],
    )
    .await
    .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    assert_eq!(project_dag(&events, &dag_id).edges.len(), 1);

    gate.mutate_with_decision(
        &dag_id,
        "delete b",
        None,
        vec![Mutation::DeleteNode {
            node_id: node_b.clone(),
        }],
    )
    .await
    .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    assert!(!dag.nodes.contains_key(&node_b));
    assert!(
        dag.edges.is_empty(),
        "edges touching the removed node are dropped"
    );
    // With the blocker gone, C becomes READY.
    assert!(ready_nodes(&dag).contains(&node_c));
}
