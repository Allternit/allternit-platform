// 0-substrate/rails/src/gate/tests.rs
#[cfg(test)]
mod autoland_tests {
    use super::super::*;
    use crate::ledger::ledger::{Ledger, LedgerOptions};
    use crate::leases::leases::{Leases, LeasesOptions};
    use crate::receipts::store::{ReceiptStore, ReceiptStoreOptions};
    use crate::core::types::{AllternitEvent, Actor, ActorType, EventScope};
    use tempfile::tempdir;
    use std::sync::Arc;
    use chrono::Utc;
    use serde_json::json;

    #[tokio::test]
    async fn test_autoland_dry_run() {
        let root = tempdir().unwrap();
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(root.path().to_path_buf()),
            ledger_dir: None,
        }));
        let leases = Arc::new(Leases::new(LeasesOptions {
            root_dir: Some(root.path().to_path_buf()),
            leases_dir: None,
            ..Default::default()
        }).await.unwrap());
        let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.path().to_path_buf()),
            receipts_dir: None,
            blobs_dir: None,
        }).unwrap());
        
        let gate = Gate::new(GateOptions {
            ledger: ledger.clone(),
            leases: leases.clone(),
            receipts: receipts.clone(),
            index: None,
            vault: None,
        oauth_vault: None,
            root_dir: Some(root.path().to_path_buf()),
            actor_id: Some("test_gate".to_string()),
            strict_provenance: Some(false),
            visual_provider: None,
            visual_config: None,
        });

        let wih_id = "test_wih";
        
        // 1. Create a "WIHCreated" event
        let created_evt = AllternitEvent {
            event_id: "evt_0".to_string(),
            ts: Utc::now().to_rfc3339(),
            actor: Actor { r#type: ActorType::Agent, id: "planner".to_string() },
            scope: Some(EventScope { wih_id: Some(wih_id.to_string()), ..Default::default() }),
            r#type: "WIHCreated".to_string(),
            payload: json!({ "wih_id": wih_id, "dag_id": "dag_1", "node_id": "node_1" }),
            provenance: None,
        };
        ledger.append(created_evt).await.unwrap();

        // 2. Create a "PASS" event in the ledger
        let pass_evt = AllternitEvent {
            event_id: "evt_1".to_string(),
            ts: Utc::now().to_rfc3339(),
            actor: Actor { r#type: ActorType::Agent, id: "validator".to_string() },
            scope: Some(EventScope { wih_id: Some(wih_id.to_string()), ..Default::default() }),
            r#type: "WIHClosedSigned".to_string(),
            payload: json!({ "wih_id": wih_id, "final_status": "PASS" }),
            provenance: None,
        };
        ledger.append(pass_evt).await.unwrap();

        // 3. Create sandbox files
        let runner_dir = root.path().join(".allternit").join("runner").join(wih_id);
        std::fs::create_dir_all(runner_dir.join("src")).unwrap();
        std::fs::write(runner_dir.join("src/test.rs"), "pub fn test() {}").unwrap();

        // 4. Execute Autoland Dry Run
        let result = gate.autoland_wih(wih_id, true, false).await.unwrap();
        
        assert!(result.dry_run);
        assert!(!result.success);
        assert_eq!(result.impact.added.len(), 1);
        assert_eq!(result.impact.added[0], "src/test.rs");
        
        // Verify root is still empty
        assert!(!root.path().join("src/test.rs").exists());
    }

    #[tokio::test]
    async fn test_autoland_full_execution() {
        let root = tempdir().unwrap();
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(root.path().to_path_buf()),
            ledger_dir: None,
        }));
        let leases = Arc::new(Leases::new(LeasesOptions {
            root_dir: Some(root.path().to_path_buf()),
            leases_dir: None,
            ..Default::default()
        }).await.unwrap());
        let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.path().to_path_buf()),
            receipts_dir: None,
            blobs_dir: None,
        }).unwrap());
        
        let gate = Gate::new(GateOptions {
            ledger: ledger.clone(),
            leases: leases.clone(),
            receipts: receipts.clone(),
            index: None,
            vault: None,
        oauth_vault: None,
            root_dir: Some(root.path().to_path_buf()),
            actor_id: Some("test_gate".to_string()),
            strict_provenance: Some(false),
            visual_provider: None,
            visual_config: None,
        });

        let wih_id = "test_wih_full";
        
        // 1. Create a "WIHCreated" event
        let created_evt = AllternitEvent {
            event_id: "evt_created_2".to_string(),
            ts: Utc::now().to_rfc3339(),
            actor: Actor { r#type: ActorType::Agent, id: "planner".to_string() },
            scope: Some(EventScope { wih_id: Some(wih_id.to_string()), ..Default::default() }),
            r#type: "WIHCreated".to_string(),
            payload: json!({ "wih_id": wih_id, "dag_id": "dag_2", "node_id": "node_2" }),
            provenance: None,
        };
        ledger.append(created_evt).await.unwrap();

        // 2. Create a "PASS" event
        let pass_evt = AllternitEvent {
            event_id: "evt_2".to_string(),
            ts: Utc::now().to_rfc3339(),
            actor: Actor { r#type: ActorType::Agent, id: "validator".to_string() },
            scope: Some(EventScope { wih_id: Some(wih_id.to_string()), ..Default::default() }),
            r#type: "WIHClosedSigned".to_string(),
            payload: json!({ "wih_id": wih_id, "final_status": "PASS" }),
            provenance: None,
        };
        ledger.append(pass_evt).await.unwrap();

        // 3. Create sandbox files
        let runner_dir = root.path().join(".allternit").join("runner").join(wih_id);
        std::fs::create_dir_all(runner_dir.join("src")).unwrap();
        std::fs::write(runner_dir.join("src/landed.rs"), "pub fn landed() {}").unwrap();

        // 4. Execute Autoland Full
        let result = gate.autoland_wih(wih_id, false, false).await.unwrap();
        
        assert!(!result.dry_run);
        assert!(result.success);
        assert_eq!(result.impact.added.len(), 1);
        
        // 5. Verify file exists in root
        assert!(root.path().join("src/landed.rs").exists());
        let content = std::fs::read_to_string(root.path().join("src/landed.rs")).unwrap();
        assert_eq!(content, "pub fn landed() {}");
        
        // 6. Verify backup was created
        assert!(root.path().join(".allternit").join("backups").exists());
    }
}

#[cfg(test)]
mod wp3_effect_tests {
    use super::super::*;
    use crate::ledger::ledger::{Ledger, LedgerOptions};
    use crate::leases::leases::{Leases, LeasesOptions};
    use crate::receipts::store::{ReceiptStore, ReceiptStoreOptions};
    use std::sync::Arc;

    #[tokio::test]
    async fn gate_tool_call_is_chained_and_duplicate_key_not_reexecuted() {
        let root = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::new(LedgerOptions { root_dir: Some(root.path().to_path_buf()), ledger_dir: None }));
        let leases = Arc::new(Leases::new(LeasesOptions {
            root_dir: Some(root.path().to_path_buf()), leases_dir: None, ..Default::default() }).await.unwrap());
        let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.path().to_path_buf()), receipts_dir: None, blobs_dir: None }).unwrap());
        let gate = Gate::new(GateOptions {
            ledger, leases, receipts: receipts.clone(), index: None, vault: None, oauth_vault: None,
            root_dir: Some(root.path().to_path_buf()), actor_id: Some("t".into()),
            strict_provenance: Some(false), visual_provider: None, visual_config: None,
        });
        let p = serde_json::json!({"idempotency_key": "gate-key-0001", "cmd": "touch x"});
        let a = gate.post_tool("w1", "shell", p.clone()).await.unwrap();
        let b = gate.post_tool("w1", "shell", p).await.unwrap();
        assert_eq!(a, b, "replay returns the recorded receipt");
        let all = receipts.query_receipts(&Default::default()).unwrap();
        assert_eq!(all.len(), 1, "duplicate key must not write a second legacy receipt");
        let rep = receipts.chain_store().unwrap().verify_chain("run_w1").unwrap();
        assert!(rep.ok && rep.length == 2, "{rep:?}");
        assert_eq!(receipts.verify_receipt(&a).unwrap().integrity, "legacy (unsigned, unchained)");
    }
}
