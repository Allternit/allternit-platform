use std::path::PathBuf;
use std::sync::Arc;

use allternit_factory_engine::core::types::{AllternitEvent, Actor, ActorType, ReceiptRecord};
use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::{
    Gate, GateOptions, Index, IndexOptions, Leases, Ledger, LedgerQuery, ReceiptStore,
    ReceiptStoreOptions, Vault, VaultOptions,
};
use allternit_factory_engine::wih::active_wihs;
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("allternit-rails-")
        .tempdir_in(base)
        .unwrap()
}

#[tokio::test]
async fn fresh_wih_writes_context_pack() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());

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

    let (_, dag_id, node_id) = gate.plan_new("Fresh Task", None).await.unwrap();
    let wih_id = gate
        .wih_pickup_with(
            &dag_id,
            &node_id,
            "agent-1",
            allternit_factory_engine::gate::gate::WihPickupOptions {
                role: None,
                fresh: true,
            },
        )
        .await
        .unwrap();

    let context_path = root
        .join(".allternit/work/dags")
        .join(&dag_id)
        .join("wih/context")
        .join(format!("{wih_id}.context.json"));
    assert!(context_path.exists());
}

#[tokio::test]
async fn plan_new_emits_required_events() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());

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

    let index = Arc::new(
        Index::new(IndexOptions {
            root_dir: Some(root.clone()),
            index_dir: Some(PathBuf::from(".allternit/index")),
        })
        .await
        .unwrap(),
    );

    let vault = Arc::new(Vault::new(VaultOptions {
        root_dir: Some(root.clone()),
        ledger: ledger.clone(),
        actor_id: Some("gate".to_string()),
    }));

    let gate = Gate::new(GateOptions {
        ledger: ledger.clone(),
        leases,
        receipts,
        index: Some(index),
        vault: Some(vault),
        oauth_vault: None,
        root_dir: Some(root.clone()),
        actor_id: Some("gate".to_string()),
        strict_provenance: None,
        visual_provider: None,
        visual_config: None,
    });

    let _ = gate.plan_new("Build ledger", None).await.unwrap();
    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    assert!(events.iter().any(|e| e.r#type == "PromptCreated"));
    assert!(events.iter().any(|e| e.r#type == "DagCreated"));
    assert!(events.iter().any(|e| e.r#type == "PromptLinkedToWork"));
}

#[tokio::test]
async fn pre_tool_denies_without_open_signature() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());

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

    let (_, dag_id, node_id) = gate.plan_new("Task", None).await.unwrap();
    let wih_id = gate.wih_pickup(&dag_id, &node_id, "agent-1").await.unwrap();
    let res = gate.pre_tool(&wih_id, "fs.write", &vec![]).await.unwrap();
    assert!(!res.allowed);
}

#[tokio::test]
async fn blocked_by_cycle_is_rejected() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());

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

    let (_, dag_id, root_node) = gate.plan_new("Root", None).await.unwrap();
    let node_b = "n_0002".to_string();
    let node_c = "n_0003".to_string();

    gate.plan_refine(
        &dag_id,
        "add nodes",
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

    let (_, mutation_ids) = gate
        .mutate_with_decision(
            &dag_id,
            "add blocked_by edges",
            None,
            vec![
                Mutation::AddBlockedBy {
                    from_node_id: root_node.clone(),
                    to_node_id: node_b.clone(),
                },
                Mutation::AddBlockedBy {
                    from_node_id: node_b.clone(),
                    to_node_id: node_c.clone(),
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(mutation_ids.len(), 2);
    let cycle = gate
        .mutate_with_decision(
            &dag_id,
            "add cycle edge",
            None,
            vec![Mutation::AddBlockedBy {
                from_node_id: node_c.clone(),
                to_node_id: root_node.clone(),
            }],
        )
        .await;
    assert!(cycle.is_err());
}

#[tokio::test]
async fn authoritative_stores_are_created() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());

    let ledger = Ledger::new(LedgerOptions {
        root_dir: Some(root.clone()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    });

    let event = AllternitEvent {
        event_id: String::new(),
        ts: String::new(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "gate".to_string(),
        },
        scope: None,
        r#type: "TestEvent".to_string(),
        payload: serde_json::json!({ "ok": true }),
        provenance: None,
    };
    ledger.append(event.clone()).await.unwrap();

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let ledger_file = root
        .join(".allternit/ledger/events")
        .join(format!("{date}.jsonl"));
    assert!(ledger_file.exists());

    let _leases = Leases::new(LeasesOptions {
        root_dir: Some(root.clone()),
        leases_dir: Some(PathBuf::from(".allternit/leases")),
        event_sink: None,
        actor_id: Some("gate".to_string()),
        auto_renewal_enabled: true,
        auto_renewal_threshold_seconds: 300,
        auto_renewal_interval_seconds: 60,
        auto_renewal_extend_seconds: 600,
    })
    .await
    .unwrap();
    let lease_db = root.join(".allternit/leases/leases.db");
    assert!(lease_db.exists());

    let receipts = ReceiptStore::new(ReceiptStoreOptions {
        root_dir: Some(root.clone()),
        receipts_dir: Some(PathBuf::from(".allternit/receipts")),
        blobs_dir: Some(PathBuf::from(".allternit/blobs")),
    })
    .unwrap();

    let receipt = ReceiptRecord {
        receipt_id: "rcpt-test".to_string(),
        run_id: "run-1".to_string(),
        step: Some(1),
        tool: "test".to_string(),
        tool_version: Some("0.1".to_string()),
        inputs_ref: Some("input".to_string()),
        outputs_ref: Some("output".to_string()),
        exit: None,
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
    };
    let receipt_path = receipts.write_receipt_with_ts(receipt.clone()).unwrap();
    assert!(receipt_path.exists());

    let blob_id = receipts.store_blob_string("blob-data").unwrap();
    assert!(receipts.blob_path(&blob_id).exists());

    assert!(receipts.receipt_path(&receipt.receipt_id).exists());
}

#[tokio::test]
async fn active_wihs_tracks_pickup_and_close() {
    let tmp = test_root();
    let root = PathBuf::from(tmp.path());

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

    let (_, dag_id, node_id) = gate.plan_new("Tracked Task", None).await.unwrap();
    let wih_id = gate.wih_pickup(&dag_id, &node_id, "agent-1").await.unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let active = active_wihs(&events);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].wih_id, wih_id);
    assert_eq!(active[0].status, "ACTIVE");

    gate.wih_close(&wih_id, "done", &["evidence".to_string()])
        .await
        .unwrap();

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    assert_eq!(active_wihs(&events).len(), 0);
}
