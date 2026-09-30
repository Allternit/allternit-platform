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

#[cfg(test)]
mod wp5_replay_gate_tests {
    use super::super::*;
    use crate::ledger::ledger::{Ledger, LedgerOptions};
    use crate::leases::leases::{Leases, LeasesOptions};
    use crate::receipts::store::{ReceiptStore, ReceiptStoreOptions};
    use crate::replay::{record_cassette, save_cassette, Boundary, DivergenceKind, Verdict};
    use serde_json::{json, Value};
    use std::sync::Arc;

    async fn gate_at(root: &std::path::Path) -> (Gate, Arc<ReceiptStore>) {
        let ledger = Arc::new(Ledger::new(LedgerOptions { root_dir: Some(root.to_path_buf()), ledger_dir: None }));
        let leases = Arc::new(Leases::new(LeasesOptions {
            root_dir: Some(root.to_path_buf()), leases_dir: None, ..Default::default() }).await.unwrap());
        let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.to_path_buf()), receipts_dir: None, blobs_dir: None }).unwrap());
        let gate = Gate::new(GateOptions {
            ledger, leases, receipts: receipts.clone(), index: None, vault: None, oauth_vault: None,
            root_dir: Some(root.to_path_buf()), actor_id: Some("t".into()),
            strict_provenance: Some(false), visual_provider: None, visual_config: None,
        });
        (gate, receipts)
    }

    fn calls() -> Vec<Value> {
        vec![
            json!({"cmd": "write a.txt", "idempotency_key": "e2e-key-write-a"}),
            json!({"cmd": "write b.txt", "effect_class": "WORKSPACE_WRITE"}),
            json!({"cmd": "write a.txt", "idempotency_key": "e2e-key-write-a"}), // reused key
            json!({"cmd": "deploy", "idempotency_key": "e2e-key-deploy-1", "effect_class": "EXTERNAL"}),
        ]
    }

    /// The agent's run: one policy decision, then side-effecting tool calls.
    async fn drive(gate: &Gate, wih: &str) -> Vec<anyhow::Result<String>> {
        gate.record_policy_decision(wih, "plan", "ALLOW").unwrap();
        let mut out = vec![];
        for p in calls() {
            out.push(gate.post_tool(wih, "shell", p).await);
        }
        out
    }

    fn keep_dir() -> (Option<tempfile::TempDir>, std::path::PathBuf) {
        match std::env::var("WP5_E2E_KEEP") {
            Ok(p) => { std::fs::create_dir_all(&p).unwrap(); (None, p.into()) }
            Err(_) => { let d = tempfile::tempdir().unwrap(); let p = d.path().to_path_buf(); (Some(d), p) }
        }
    }

    #[tokio::test]
    async fn replay_gate_run_end_to_end_zero_divergence_then_tamper() {
        let (_guard, root) = keep_dir();
        let (gate, receipts) = gate_at(&root).await;
        let live = drive(&gate, "e2e").await;
        let live: Vec<String> = live.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(live[0], live[2], "reused key deduped live");
        let cs = receipts.chain_store().unwrap();
        let c = record_cassette(&cs, "run_e2e", Some("graph_e2e"), 1).unwrap();
        // policy + 3 distinct effects (the reused key produced no second receipt)
        let kinds: Vec<_> = c.entries.iter().map(|e| e.boundary).collect();
        assert_eq!(kinds, vec![Boundary::Policy, Boundary::Tool, Boundary::Tool, Boundary::Tool]);
        save_cassette(&receipts.receipts_dir().join("_cassettes"), &c).unwrap();

        let legacy = receipts.query_receipts(&Default::default()).unwrap().len();
        let chain_len = cs.read_run("run_e2e").unwrap().len();

        // Replay through the gate: same agent behaviour, recorded_only.
        gate.begin_replay("e2e_r1", c.clone()).unwrap();
        assert!(gate.is_replaying("e2e_r1"));
        let replayed: Vec<String> = drive(&gate, "e2e_r1").await.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(replayed, live, "recorded results served");
        let rep = gate.end_replay("e2e_r1").unwrap();
        assert_eq!(rep.verdict, Verdict::Identical, "{rep:?}");
        assert!(!gate.is_replaying("e2e_r1"));
        assert_eq!(receipts.query_receipts(&Default::default()).unwrap().len(), legacy, "no live effect ran");
        assert_eq!(cs.read_run("run_e2e").unwrap().len(), chain_len, "nothing appended to the recorded chain");
        assert!(cs.read_run("run_e2e_r1").unwrap().is_empty(), "replay run has no chain of its own");

        // An unrecorded effect in replay mode is refused, not executed.
        gate.begin_replay("e2e_r2", c.clone()).unwrap();
        gate.record_policy_decision("e2e_r2", "plan", "DENY").unwrap();
        let err = gate.post_tool("e2e_r2", "shell", json!({"cmd": "rm -rf /"})).await.unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
        let rep = gate.end_replay("e2e_r2").unwrap();
        let k: Vec<_> = rep.divergences.iter().map(|d| d.kind).collect();
        assert!(k.contains(&DivergenceKind::PolicyOutcome) && k.contains(&DivergenceKind::ExtraEntry), "{k:?}");
        assert_eq!(receipts.query_receipts(&Default::default()).unwrap().len(), legacy);

        if std::env::var("WP5_E2E_KEEP").is_ok() {
            return; // leave the untampered run for the CLI end-to-end check
        }
        // Tamper the second tool call's recorded receipt: divergence at that step.
        let target = &c.entries[2];
        let all = cs.read_run("run_e2e").unwrap();
        let idx = all.iter().position(|r| r["chain"]["receipt_id"] == json!(target.result_ref)).unwrap();
        let p = receipts.receipts_dir().join(format!("_chains/run_e2e/{idx:010}.json"));
        let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        v["external_ref"] = json!("forged-ref");
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        gate.begin_replay("e2e_r3", c.clone()).unwrap();
        let out = drive(&gate, "e2e_r3").await;
        // The chain no longer verifies: every effect is refused, not only the tampered one (#12).
        assert!(out.iter().all(|o| o.is_err()), "unverified chain served nothing: {out:?}");
        let rep = gate.end_replay("e2e_r3").unwrap();
        assert_eq!(rep.verdict, Verdict::UnexpectedDivergence);
        assert!(rep.divergences.iter().any(|d| d.kind == DivergenceKind::ResultHash && d.seq == target.seq), "{rep:?}");
        assert!(rep.divergences.iter().any(|d| d.node_id == "chain" && d.seq == idx as u64), "chain break at the tampered receipt: {rep:?}");
        assert_eq!(receipts.query_receipts(&Default::default()).unwrap().len(), legacy);
    }

    #[tokio::test]
    async fn wp10_replay_boundary_serves_recorded_and_refuses_unrecorded() {
        let d = tempfile::tempdir().unwrap();
        let (gate, receipts) = gate_at(d.path()).await;
        drive(&gate, "rb").await;
        let c = record_cassette(&receipts.chain_store().unwrap(), "run_rb", None, 0).unwrap();
        let steps = crate::replay::ReplayStep::from_cassette(&c);
        assert!(gate.replay_boundary("rb_r", &steps[0]).is_err(), "not replaying");
        gate.begin_replay("rb_r", c.clone()).unwrap();
        gate.replay_boundary("rb_r", &steps[0]).unwrap();
        let mut bogus = steps[0].clone();
        bogus.node_id = "unrecorded".into();
        bogus.request_hash = crate::replay::boundary_request_hash(Boundary::Policy, "unrecorded", "x").unwrap();
        assert!(gate.replay_boundary("rb_r", &bogus).unwrap_err().to_string().contains("refused"));
        assert_eq!(receipts.chain_store().unwrap().read_run("run_rb_r").unwrap().len(), 0, "no evidence appended");
        gate.end_replay("rb_r").unwrap();
    }

    #[tokio::test]
    async fn review10_gate_reserve_before_effect_runs_it_once() {
        use crate::receipts::store::ToolEffectAdmission;
        let d = tempfile::tempdir().unwrap();
        let (gate, _r) = gate_at(d.path()).await;
        let p = json!({"cmd": "charge card", "idempotency_key": "gate-pay-0001", "effect_class": "FINANCIAL"});
        let mut effects = 0;
        // Runtime A and runtime B both reach pre-tool admission before either executes.
        let a = gate.reserve_tool_effect("rv", "shell", &p).unwrap();
        assert!(matches!(a, ToolEffectAdmission::Reserved(_)));
        assert!(gate.reserve_tool_effect("rv", "shell", &p).is_err(), "duplicate admitted before first effect");
        effects += 1; // only A executes
        let id = gate.post_tool("rv", "shell", p.clone()).await.unwrap();
        // A later retry is answered from the record; the tool does not run.
        assert_eq!(gate.reserve_tool_effect("rv", "shell", &p).unwrap(), ToolEffectAdmission::AlreadyCommitted(id.clone()));
        assert_eq!(gate.post_tool("rv", "shell", p.clone()).await.unwrap(), id);
        assert_eq!(effects, 1);
        // Same key, different operation: conflict, not a replayed result (#15).
        let other = json!({"cmd": "publish", "idempotency_key": "gate-pay-0001", "effect_class": "FINANCIAL"});
        assert!(gate.reserve_tool_effect("rv", "shell", &other).unwrap_err().to_string().contains("conflict"));
    }

    #[tokio::test]
    async fn review11_17_wrong_gate_cannot_end_replay_or_append() {
        let d = tempfile::tempdir().unwrap();
        let (a, receipts) = gate_at(d.path()).await;
        let (b, _) = gate_at(d.path()).await; // shares A's receipts root, no in-memory session
        drive(&a, "rec").await;
        let c = record_cassette(&receipts.chain_store().unwrap(), "run_rec", None, 0).unwrap();
        a.begin_replay("w", c.clone()).unwrap();
        // #11: B cannot end A's replay, and the shared marker survives.
        assert!(b.end_replay("w").is_err());
        assert!(b.is_replaying("w"), "live protection marker deleted by the wrong gate");
        assert!(b.begin_replay("w", c.clone()).is_err(), "second session took over the run");
        assert!(b.abort_replay("w", "rpl_wrong").is_err());
        assert!(b.is_replaying("w"));
        // #17: nothing live is appended during recorded-only replay.
        let cs = receipts.chain_store().unwrap();
        let (chain_len, legacy) = (cs.read_run("run_w").unwrap().len(), receipts.query_receipts(&Default::default()).unwrap().len());
        assert!(a.post_tool("w", "shell", json!({"cmd": "cat x", "effect_class": "READ"})).await.is_err());
        assert!(b.post_tool("w", "shell", json!({"cmd": "cat x", "effect_class": "READ"})).await.is_err());
        assert!(b.record_policy_decision("w", "plan", "ALLOW").is_err());
        assert_eq!(cs.read_run("run_w").unwrap().len(), chain_len, "policy/read state appended during replay");
        assert_eq!(receipts.query_receipts(&Default::default()).unwrap().len(), legacy, "read receipt written during replay");
        // The owning session ends it; the token-bound abort path also works.
        a.end_replay("w").unwrap();
        assert!(!b.is_replaying("w"));
        a.begin_replay("w2", c).unwrap();
        let tok = a.replay_session_token("w2").unwrap();
        b.abort_replay("w2", &tok).unwrap();
        assert!(!b.is_replaying("w2"));
    }
}
