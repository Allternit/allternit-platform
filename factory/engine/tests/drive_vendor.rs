//! `drive` with vendor bots (`--team`): a `bot:<slug>` node whose bot is a
//! vendor bot is picked up for `bot:<slug>` and delivered as a vendor ticket
//! through allternit-api (a fake here). The open WIH then waits on the ticket;
//! a refused ticket is a needs-you gate, retried only after a person resolves it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use allternit_factory_engine::agents::team_apply::{ApiError, FactoryApi};
use allternit_factory_engine::drive::hooks::NoHooks;
use allternit_factory_engine::drive::{
    DriveOptions, DriveReport, Driver, VendorInfo, BOT_NOTIFIED, NEEDS_YOU, VENDOR_TICKET_CREATED,
    VENDOR_TICKET_FAILED,
};
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::templates::{builtin_template, plan_from_template_with_roles, RoleMap};
use allternit_factory_engine::wait_gates::GateOutcome;
use allternit_factory_engine::work::{project_dag, DagState};
use allternit_factory_engine::workspace::node_page;
use allternit_factory_engine::{
    Actor, ActorType, AllternitEvent, Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore,
    ReceiptStoreOptions,
};
use serde_json::{json, Value};
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    let dir = tempfile::Builder::new().prefix("drive-vendor-").tempdir_in(base).unwrap();
    // Capacity admission must not depend on how busy the test machine is.
    let drive = dir.path().join(".allternit/drive");
    std::fs::create_dir_all(&drive).unwrap();
    std::fs::write(
        drive.join("config.json"),
        json!({ "min_free_mem_mb": 0, "max_load_per_cpu": 100000.0, "poll_interval_ms": 50 }).to_string(),
    )
    .unwrap();
    dir
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

struct FakeApi {
    tickets: Mutex<Vec<Value>>,
    fail: Mutex<bool>,
}

impl FactoryApi for FakeApi {
    fn upsert_bot(&self, _: &Value) -> Result<Value, ApiError> {
        unreachable!("drive never registers bots")
    }
    fn list_bots(&self) -> Result<Vec<Value>, ApiError> {
        Ok(vec![])
    }
    fn create_node_ticket(&self, body: &Value) -> Result<Value, ApiError> {
        self.tickets.lock().unwrap().push(body.clone());
        if *self.fail.lock().unwrap() {
            return Err(ApiError::new("usage", Some(400), "That vendor bot has no thread yet. Open one, or pass threadId."));
        }
        Ok(json!({ "ticket": "T-7", "ticketId": "T-7", "lane": "channel", "guarantee": "best_effort", "created": true, "nudgeSent": true }))
    }
}

async fn events(ledger: &Ledger, ty: &str) -> Vec<AllternitEvent> {
    ledger.query(LedgerQuery { r#type: Some(ty.to_string()), ..Default::default() }).await.unwrap()
}

async fn dag(ledger: &Ledger, dag_id: &str) -> DagState {
    let all = ledger.query(LedgerQuery::default()).await.unwrap();
    let evs: Vec<AllternitEvent> = all
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(Value::as_str) == Some(dag_id))
        .collect();
    project_dag(&evs, dag_id)
}

async fn drive_once(root: &Path, ledger: &Arc<Ledger>, gate: &Arc<Gate>, dag_id: &str, api: &Arc<FakeApi>) -> DriveReport {
    let mut vendor_bots = BTreeMap::new();
    vendor_bots.insert(
        "research".to_string(),
        VendorInfo { vendor: Some("chatgpt".into()), lane: Some("official".into()), team: Some("product-build".into()) },
    );
    let mut driver = Driver::new(
        root.to_path_buf(),
        ledger.clone(),
        Some(gate.clone()),
        DriveOptions { dag_id: dag_id.to_string(), once: true, vendor_bots, ..Default::default() },
        Arc::new(NoHooks),
    )
    .unwrap()
    .with_vendor_api(api.clone() as Arc<dyn FactoryApi>);
    driver.run(std::future::pending::<()>()).await.unwrap()
}

async fn plan(gate: &Gate) -> (String, String) {
    let template = builtin_template("build-check-prove").unwrap();
    let mut roles = RoleMap::new();
    roles.insert("build".into(), "bot:research".into());
    roles.insert("check".into(), "bot:checker".into());
    let params = HashMap::from([("intent".to_string(), "a market summary".to_string())]);
    let p = plan_from_template_with_roles(gate, &template, &params, None, None, None, Some(&roles)).await.unwrap();
    (p.dag_id, p.nodes["build"].clone())
}

#[tokio::test]
async fn vendor_bot_nodes_become_tickets_and_wait_on_them() {
    let tmp = test_root();
    let root = tmp.path();
    let (ledger, gate) = build_gate(root).await;
    let (dag_id, build) = plan(&gate).await;
    let api = Arc::new(FakeApi { tickets: Mutex::new(vec![]), fail: Mutex::new(false) });

    let report = drive_once(root, &ledger, &gate, &dag_id, &api).await;
    assert_eq!(report.vendor_tickets, vec![(build.clone(), "T-7".to_string(), "best_effort".to_string())]);
    assert!(report.spawned.is_empty() && report.bot_notified.is_empty());
    let sent = api.tickets.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["botSlug"], "research");
    assert_eq!(sent[0]["dagId"], dag_id.as_str());
    assert_eq!(sent[0]["nodeId"], build.as_str());
    assert!(sent[0]["instructions"].as_str().unwrap().contains("a market summary"));

    let d = dag(&ledger, &dag_id).await;
    let wih = d.nodes[&build].current_wih_id.clone().expect("picked up");
    assert_eq!(d.nodes[&build].assignee.as_deref(), Some("bot:research"));
    assert_eq!(sent[0]["wihId"], wih.as_str());
    let created = events(&ledger, VENDOR_TICKET_CREATED).await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].payload["ticket"], "T-7");
    assert_eq!(created[0].payload["guarantee"], "best_effort");
    assert_eq!(created[0].payload["to"], "research@product-build");
    assert!(events(&ledger, BOT_NOTIFIED).await.is_empty(), "no mail fallback");

    // Second pass: waiting on the ticket, nothing re-sent or respawned.
    let again = drive_once(root, &ledger, &gate, &dag_id, &api).await;
    assert_eq!(api.tickets.lock().unwrap().len(), 1);
    assert!(again.vendor_tickets.is_empty() && again.needs_you.is_empty() && again.spawned.is_empty());
    assert!(
        again.waiting.iter().any(|l| l.contains("waiting on vendor ticket T-7")),
        "{:?}",
        again.waiting
    );

    // The node page lists the ticket as a delivery (pure function of the ledger).
    let all = ledger.query(LedgerQuery::default()).await.unwrap();
    let page = node_page::build(root, &all, &dag_id, &build).unwrap();
    assert_eq!(page.deliveries.len(), 1);
    let v = serde_json::to_value(&page.deliveries[0]).unwrap();
    assert_eq!(v["via"], "vendor_ticket");
    assert_eq!(v["state"], "best_effort");
    assert_eq!(v["ticket"], "T-7");
    assert_eq!(v["dagId"], dag_id.as_str());
    assert_eq!(v["to"], "research@product-build");
}

#[tokio::test]
async fn refused_ticket_needs_you_and_retries_only_after_resolution() {
    let tmp = test_root();
    let root = tmp.path();
    let (ledger, gate) = build_gate(root).await;
    let (dag_id, build) = plan(&gate).await;
    let api = Arc::new(FakeApi { tickets: Mutex::new(vec![]), fail: Mutex::new(true) });

    let report = drive_once(root, &ledger, &gate, &dag_id, &api).await;
    assert_eq!(report.needs_you, vec![(build.clone(), VENDOR_TICKET_FAILED.to_string())]);
    assert!(events(&ledger, VENDOR_TICKET_CREATED).await.is_empty());
    let needs = events(&ledger, NEEDS_YOU).await;
    assert!(needs[0].payload["detail"].as_str().unwrap().contains("no thread yet"), "{:?}", needs[0].payload);

    // Unresolved: not retried.
    drive_once(root, &ledger, &gate, &dag_id, &api).await;
    assert_eq!(api.tickets.lock().unwrap().len(), 1);

    // A person resolves the gate after fixing the cause: the ticket is created on the same WIH.
    *api.fail.lock().unwrap() = false;
    let d = dag(&ledger, &dag_id).await;
    let wih = d.nodes[&build].current_wih_id.clone().unwrap();
    let g = d.nodes[&build].wait_gates.iter().find(|g| g.params.get("reason").and_then(Value::as_str) == Some(VENDOR_TICKET_FAILED)).unwrap().clone();
    gate.resolve_node_wait_gate(&dag_id, &build, &g.gate_id, GateOutcome::Ok, Some(Actor { r#type: ActorType::User, id: "eoj".into() }), None)
        .await
        .unwrap();
    let report = drive_once(root, &ledger, &gate, &dag_id, &api).await;
    assert_eq!(report.vendor_tickets.len(), 1, "{report:?}");
    let sent = api.tickets.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["wihId"], wih.as_str());
}
