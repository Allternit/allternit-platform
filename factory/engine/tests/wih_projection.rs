use std::path::PathBuf;
use std::sync::Arc;

use allternit_commrails::work::project_dag;
use allternit_commrails::leases::leases::LeasesOptions;
use allternit_commrails::ledger::ledger::LedgerOptions;
use allternit_commrails::{
    Actor, ActorType, AllternitEvent, Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore,
    ReceiptStoreOptions,
};
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
async fn wih_pickup_sets_current_wih_id_and_assignee_on_node() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (_, dag_id, root_node) = gate.plan_new("Root", None).await.unwrap();

    let wih_id = gate
        .wih_pickup(&dag_id, &root_node, "agent-1")
        .await
        .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    let node = dag.nodes.get(&root_node).expect("root node projected");
    assert_eq!(node.current_wih_id.as_deref(), Some(wih_id.as_str()));
    assert_eq!(node.assignee.as_deref(), Some("agent-1"));
}

#[tokio::test]
async fn wih_close_clears_current_wih_id_and_assignee_on_node() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (_, dag_id, root_node) = gate.plan_new("Root", None).await.unwrap();
    let wih_id = gate
        .wih_pickup(&dag_id, &root_node, "agent-1")
        .await
        .unwrap();

    gate.wih_close(&wih_id, "PASS", &["ev-1".to_string()])
        .await
        .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    let node = dag.nodes.get(&root_node).expect("root node projected");
    assert_eq!(node.current_wih_id, None);
    assert_eq!(node.assignee, None);
}

#[tokio::test]
async fn unenriched_pickup_resolves_node_via_wih_created_fallback() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (_, dag_id, root_node) = gate.plan_new("Root", None).await.unwrap();

    // Hand-craft pre-v5 shaped WIH events: pickup carries no dag_id/node_id,
    // so the projection must resolve the node via the WIHCreated fallback map.
    let created = AllternitEvent {
        event_id: "evt_99999999999999_000001".to_string(),
        ts: "2099-01-01T00:00:00Z".to_string(),
        actor: Actor {
            r#type: ActorType::Agent,
            id: "legacy-agent".to_string(),
        },
        scope: None,
        r#type: "WIHCreated".to_string(),
        payload: serde_json::json!({
            "wih_id": "wih_legacy",
            "dag_id": dag_id,
            "node_id": root_node,
            "execution_mode": "shared"
        }),
        provenance: None,
    };
    let picked = AllternitEvent {
        event_id: "evt_99999999999999_000002".to_string(),
        ts: "2099-01-01T00:01:00Z".to_string(),
        actor: Actor {
            r#type: ActorType::Agent,
            id: "legacy-agent".to_string(),
        },
        scope: None,
        r#type: "WIHPickedUp".to_string(),
        payload: serde_json::json!({
            "wih_id": "wih_legacy",
            "agent_id": "agent-9",
            "picked_up_at": "2099-01-01T00:01:00Z"
        }),
        provenance: None,
    };
    ledger.append(created).await.unwrap();
    ledger.append(picked).await.unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    let node = dag.nodes.get(&root_node).expect("root node projected");
    assert_eq!(node.current_wih_id.as_deref(), Some("wih_legacy"));
    assert_eq!(node.assignee.as_deref(), Some("agent-9"));
}

#[tokio::test]
async fn unenriched_close_clears_node_via_wih_created_fallback() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());
    let (ledger, gate) = build_gate(&root).await;

    let (_, dag_id, root_node) = gate.plan_new("Root", None).await.unwrap();

    let created = AllternitEvent {
        event_id: "evt_99999999999999_000001".to_string(),
        ts: "2099-01-01T00:00:00Z".to_string(),
        actor: Actor {
            r#type: ActorType::Agent,
            id: "legacy-agent".to_string(),
        },
        scope: None,
        r#type: "WIHCreated".to_string(),
        payload: serde_json::json!({
            "wih_id": "wih_legacy",
            "dag_id": dag_id,
            "node_id": root_node,
            "execution_mode": "shared"
        }),
        provenance: None,
    };
    let picked = AllternitEvent {
        event_id: "evt_99999999999999_000002".to_string(),
        ts: "2099-01-01T00:01:00Z".to_string(),
        actor: Actor {
            r#type: ActorType::Agent,
            id: "legacy-agent".to_string(),
        },
        scope: None,
        r#type: "WIHPickedUp".to_string(),
        payload: serde_json::json!({
            "wih_id": "wih_legacy",
            "agent_id": "agent-9",
            "picked_up_at": "2099-01-01T00:01:00Z"
        }),
        provenance: None,
    };
    let closed = AllternitEvent {
        event_id: "evt_99999999999999_000003".to_string(),
        ts: "2099-01-01T00:02:00Z".to_string(),
        actor: Actor {
            r#type: ActorType::Agent,
            id: "legacy-agent".to_string(),
        },
        scope: None,
        r#type: "WIHClosedSigned".to_string(),
        payload: serde_json::json!({
            "wih_id": "wih_legacy",
            "final_status": "PASS",
            "closed_at": "2099-01-01T00:02:00Z"
        }),
        provenance: None,
    };
    ledger.append(created).await.unwrap();
    ledger.append(picked).await.unwrap();
    ledger.append(closed).await.unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let dag = project_dag(&events, &dag_id);
    let node = dag.nodes.get(&root_node).expect("root node projected");
    assert_eq!(node.current_wih_id, None);
    assert_eq!(node.assignee, None);
}
