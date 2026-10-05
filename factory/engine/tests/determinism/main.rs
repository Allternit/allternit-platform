//! The determinism contract (SPEC §6), tested on the engine library:
//!
//! - replay: throwing a projection away and rebuilding it from the ledger
//!   gives the same projection (DAG cards, deliveries, the agent view);
//! - `--dry-run` lists exactly the ledger records the real run then writes;
//! - idempotent create: the same key twice makes one thing;
//! - the exit-code table: every API.md error code maps to one HTTP status;
//! - one registry, reconciled: a session whose pane is gone reads dead.
//!
//! The CLI half of the exit-code table and the HTTP contract run against the
//! real binary in `cmd/allternit-factory/tests/factory_api.rs`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use allternit_factory_engine::api::factory::{cards, error};
use allternit_factory_engine::backend::{self, LivePane, PaneBackend, PaneSend, PaneSpawn};
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::registry::{reconcile_file, Entry, Registry, RegistryFile};
use allternit_factory_engine::send::{self, ApiLink, DeliveryFilter, SendCtx, SendRequest, DELIVERY_EVENT};
use allternit_factory_engine::templates::{parse_markdown_template, plan_from_template};
use allternit_factory_engine::view;
use allternit_factory_engine::work::project_dag;
use allternit_factory_engine::{
    Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore, ReceiptStoreOptions,
};
use serde_json::Value;
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::Builder::new().prefix("f3det").tempdir_in("/tmp").unwrap()
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

/// Panes as a plain list the test controls. Sends to a live pane verify.
#[derive(Default)]
struct Panes {
    live: Mutex<Vec<LivePane>>,
}

impl PaneBackend for Panes {
    fn spawn(&self, req: &PaneSpawn) -> anyhow::Result<LivePane> {
        let p = LivePane { session: req.session.clone(), pane_id: format!("p-{}", req.session), cwd: None, agent_status: Some("idle".into()) };
        self.live.lock().unwrap().push(p.clone());
        Ok(p)
    }
    fn list(&self) -> anyhow::Result<Vec<LivePane>> {
        Ok(self.live.lock().unwrap().clone())
    }
    fn send(&self, _root: &Path, session: &str, _text: &str, _sender: &str, queue_only: bool) -> anyhow::Result<PaneSend> {
        let live = self.live.lock().unwrap().iter().any(|p| p.session == session);
        Ok(if live && !queue_only {
            PaneSend::Verified
        } else {
            PaneSend::Queued { message_id: "1".into(), depth: 1, reason: "the pane is not running".into() }
        })
    }
    fn capture(&self, _session: &str, _lines: u32) -> anyhow::Result<String> {
        Ok(String::new())
    }
    fn kill(&self, session: &str) -> anyhow::Result<()> {
        self.live.lock().unwrap().retain(|p| p.session != session);
        Ok(())
    }
}

/// One pane backend per test process (it's process-wide); tests use
/// distinct session names, so they don't see each other's panes as theirs.
fn panes() -> Arc<Panes> {
    static P: OnceLock<Arc<Panes>> = OnceLock::new();
    P.get_or_init(|| {
        let p = Arc::new(Panes::default());
        backend::install(p.clone());
        p
    })
    .clone()
}

fn live(session: &str) -> LivePane {
    LivePane { session: session.into(), pane_id: format!("p-{session}"), cwd: None, agent_status: Some("idle".into()) }
}

fn registry_with(dir: &Path, sessions: &[&str]) -> Registry {
    let reg = Registry::at(dir.join("registry.json"));
    for s in sessions {
        reg.upsert(s, Entry { lifecycle: Some("running".into()), harness: Some("claude".into()), ..Default::default() }).unwrap();
    }
    reg
}

fn ctx(root: &Path, registry: Registry) -> SendCtx {
    SendCtx { root: root.to_path_buf(), registry, api: ApiLink::default(), sender: "user:eoj".into() }
}

async fn types(ledger: &Ledger) -> Vec<String> {
    ledger.query(LedgerQuery::default()).await.unwrap().into_iter().map(|e| e.r#type).collect()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

const TEMPLATE: &str = r#"---
name: Build check prove
description: build -> check -> approve
---

```yaml template-spec
steps:
  - id: build
    title: Build
    executor: "ao:claude"
  - id: check
    title: Check
    blocked_by: [build]
  - id: approve
    title: Approve
    blocked_by: [check]
    wait_gate:
      kind: manual
      description: "A person approves"
```
"#;

// ---------------------------------------------------------------- replay

#[tokio::test]
async fn replay_rebuilds_the_same_projection() {
    let t = tmp();
    let root = t.path().join("ws");
    let (ledger, gate) = build_gate(&root).await;
    let template = parse_markdown_template("bcp", TEMPLATE).unwrap();
    let run = plan_from_template(&gate, &template, &HashMap::new(), Some("Ship it"), None).await.unwrap();
    let reg = registry_with(t.path(), &["ao-det-replay"]);
    panes().live.lock().unwrap().push(live("ao-det-replay"));
    let d = send::send(&ctx(&root, reg.clone()), &SendRequest { to: "det-replay".into(), text: "go".into(), node_id: Some("n".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(d.state, "verified");

    let events = ledger.query(LedgerQuery::default()).await.unwrap();
    let first = cards(&project_dag(&events, &run.dag_id), &events);
    let deliveries = send::deliveries(&root, &DeliveryFilter::default()).await.unwrap();

    // Throw every derived view away: a copy of the ledger alone, in a new
    // workspace, rebuilds the same projections.
    let other = t.path().join("replayed");
    copy_dir(&root.join(".allternit/ledger"), &other.join(".allternit/ledger"));
    let (ledger2, _) = build_gate(&other).await;
    let events2 = ledger2.query(LedgerQuery::default()).await.unwrap();
    assert_eq!(cards(&project_dag(&events2, &run.dag_id), &events2), first);
    assert_eq!(send::deliveries(&other, &DeliveryFilter::default()).await.unwrap(), deliveries);
    assert_eq!(first.len(), 3);

    // The agent view is a pure function of (registry, panes, peers).
    let file = reg.load().unwrap();
    let panes_now = vec![live("ao-det-replay")];
    assert_eq!(view::agents(&file, &panes_now, &[]), view::agents(&file, &panes_now, &[]));
}

// ---------------------------------------------------------------- dry run

#[tokio::test]
async fn dry_run_lists_exactly_what_the_real_send_writes() {
    let t = tmp();
    let root = t.path().join("ws");
    let (ledger, _gate) = build_gate(&root).await;
    let reg = registry_with(t.path(), &["ao-det-dry"]);
    panes().live.lock().unwrap().push(live("ao-det-dry"));
    let c = ctx(&root, reg);
    let req = SendRequest { to: "det-dry".into(), text: "check".into(), ..Default::default() };

    let before = types(&ledger).await;
    let plan = send::plan(&c, &req).await.unwrap();
    assert_eq!(types(&ledger).await, before, "a dry run wrote to the ledger");
    assert_eq!(plan.via, "pane");

    let d = send::send(&c, &req).await.unwrap();
    let after = types(&ledger).await;
    // The first send on a thread also opens it (ThreadCreated, once).
    let written: Vec<String> = after[before.len()..].iter().filter(|t| *t != "ThreadCreated").cloned().collect();
    assert_eq!(written, plan.records);
    assert_eq!(d.via, plan.via);
    assert_eq!(d.to, plan.to);
}

// ---------------------------------------------------------------- idempotent create

#[tokio::test]
async fn the_same_key_twice_makes_one_delivery() {
    let t = tmp();
    let root = t.path().join("ws");
    let (ledger, _gate) = build_gate(&root).await;
    let c = ctx(&root, registry_with(t.path(), &["ao-det-key"]));
    let req = SendRequest { to: "det-key".into(), text: "once".into(), idempotency_key: Some("k1".into()), ..Default::default() };
    let a = send::send(&c, &req).await.unwrap();
    let n = types(&ledger).await.len();
    let b = send::send(&c, &req).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(types(&ledger).await.len(), n, "the repeat wrote again");
    let all = ledger.query(LedgerQuery { r#type: Some(DELIVERY_EVENT.into()), ..Default::default() }).await.unwrap();
    assert_eq!(all.len(), 1);
    // A different key is a different delivery.
    let c2 = send::send(&c, &SendRequest { idempotency_key: Some("k2".into()), ..req }).await.unwrap();
    assert_ne!(c2.id, a.id);
}

#[tokio::test]
async fn a_campaign_is_declared_once() {
    use allternit_factory_engine::campaign::{CampaignDefinition, CampaignOps, DEFAULT_CHECK_CEILING_SECS};
    let t = tmp();
    let root = t.path().join("ws");
    let (ledger, _gate) = build_gate(&root).await;
    let ops = CampaignOps::new(root.clone(), ledger.clone(), DEFAULT_CHECK_CEILING_SECS, Default::default());
    let def: CampaignDefinition = serde_json::from_value(serde_json::json!({
        "id": "c1", "objective": "Saved views", "owner": "eoj", "executor": "bot:al"
    }))
    .unwrap();
    ops.declare(def.clone(), chrono::Utc::now()).await.unwrap();
    assert!(ops.declare(def, chrono::Utc::now()).await.is_err());
    assert_eq!(types(&ledger).await.iter().filter(|t| *t == "CampaignDeclared").count(), 1);
}

// ---------------------------------------------------------------- exit codes

#[tokio::test]
async fn every_error_code_maps_to_one_http_status() {
    let table: BTreeMap<&str, u16> = [
        ("refused", 403),
        ("not_found", 404),
        ("transport", 502),
        ("timeout", 504),
        ("needs_person", 409),
        ("usage", 400),
    ]
    .into_iter()
    .collect();
    for (code, status) in table {
        let resp = error(code, "fact", "action");
        assert_eq!(resp.status().as_u16(), status, "{code}");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], code);
        assert_eq!((v["error"]["fact"].as_str(), v["error"]["action"].as_str()), (Some("fact"), Some("action")));
    }
    // A send that can't be attempted says why, with the contract's code.
    let t = tmp();
    let root = t.path().join("ws");
    let c = ctx(&root, registry_with(t.path(), &[]));
    let e = send::send(&c, &SendRequest { to: "nobody".into(), text: "hi".into(), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.code, "not_found");
    let e = send::send(&c, &SendRequest { to: "x".into(), text: " ".into(), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.code, "usage");
}

// ---------------------------------------------------------------- the registry

#[tokio::test]
async fn the_registry_marks_dead_panes_dead() {
    let t = tmp();
    let root = t.path().join("ws");
    let reg = registry_with(t.path(), &["ao-det-gone", "ao-det-alive"]);
    panes().live.lock().unwrap().push(live("ao-det-alive"));

    let snap = view::snapshot(&root, &reg, None).await.unwrap();
    let state = |slug: &str| snap.agents.iter().find(|a| a.slug == slug).map(|a| a.state.clone());
    assert_eq!(state("det-gone").as_deref(), Some("offline"));
    assert_eq!(state("det-alive").as_deref(), Some("idle"));
    let file = reg.load().unwrap();
    assert!(file.sessions["ao-det-gone"].dead, "a gone pane must be recorded dead");
    assert_eq!(file.sessions["ao-det-gone"].lifecycle.as_deref(), Some("dead"));
    assert!(!file.sessions["ao-det-alive"].dead);
    // Every pane maps to a bot.
    assert!(file.sessions.values().all(|e| e.bot.is_some()));

    // The pane dies: the next look says so.
    backend::backend().unwrap().kill("ao-det-alive").unwrap();
    let snap = view::snapshot(&root, &reg, None).await.unwrap();
    assert_eq!(snap.agents.iter().find(|a| a.slug == "det-alive").unwrap().state, "offline");
    assert!(reg.load().unwrap().sessions["ao-det-alive"].dead);
    // A send to it now queues and says so; it never claims a paste.
    let d = send::send(&ctx(&root, reg), &SendRequest { to: "det-alive".into(), text: "hi".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!((d.via.as_str(), d.state.as_str()), ("pane_queue", "queued"));

    // Reconciling the same reality twice changes nothing the second time.
    let mut f = RegistryFile::default();
    f.sessions.insert("ao-x".into(), Entry { lifecycle: Some("running".into()), ..Default::default() });
    assert!(!reconcile_file(&mut f, &[], "t").is_empty());
    assert!(reconcile_file(&mut f, &[], "t2").is_empty());
}
