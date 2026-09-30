//! Campaigns, the keyed wake queue, the sweep lock, and the attention gate
//! wired together (unit-level policy tests live next to the modules).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_commrails::attention::{open_needs_you, AttentionConfig, QuietHoursConfig};
use allternit_commrails::campaign::{
    BudgetDecl, BudgetMode, CampaignDefinition, CampaignOps, CampaignStatus, Rearm, SpendEntry,
};
use allternit_commrails::gate::gate::DagMutation as Mutation;
use allternit_commrails::leases::leases::LeasesOptions;
use allternit_commrails::ledger::ledger::LedgerOptions;
use allternit_commrails::wait_gates::WaitGateKind;
use allternit_commrails::wake::runner::{run_due, NodeTimerHandler, SweepContext};
use allternit_commrails::wake::{self, AutomationConfig, WakeQueue};
use allternit_commrails::{
    Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore, ReceiptStoreOptions,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("allternit-campaigns-")
        .tempdir_in(base)
        .unwrap()
}

fn ledger(root: &Path) -> Arc<Ledger> {
    Arc::new(Ledger::new(LedgerOptions {
        root_dir: Some(root.to_path_buf()),
        ledger_dir: Some(PathBuf::from(".allternit/ledger")),
    }))
}

fn ops(root: &Path, l: &Arc<Ledger>) -> CampaignOps {
    CampaignOps::new(root, l.clone(), 3600, AttentionConfig::default())
}

fn def(id: &str, executor: &str, command: Option<&str>) -> CampaignDefinition {
    CampaignDefinition {
        id: id.into(),
        objective: format!("objective for {id}"),
        owner: "eoj".into(),
        status: CampaignStatus::Active,
        executor: executor.into(),
        command: command.map(String::from),
        budget: None,
        dag_id: None,
        rearm: None,
    }
}

fn ctx<'a>(
    root: &Path,
    l: &Arc<Ledger>,
    config: AutomationConfig,
    h: Option<&'a dyn NodeTimerHandler>,
) -> SweepContext<'a> {
    SweepContext {
        root: root.to_path_buf(),
        ledger: l.clone(),
        config,
        node_handler: h,
    }
}

async fn events_of(l: &Ledger, t: &str) -> Vec<serde_json::Value> {
    l.query(LedgerQuery {
        r#type: Some(t.into()),
        ..Default::default()
    })
    .await
    .unwrap()
    .into_iter()
    .map(|e| e.payload)
    .collect()
}

#[tokio::test]
async fn check_later_rearm_replaces_and_clamps() {
    let tmp = test_root();
    let l = ledger(tmp.path());
    let o = ops(tmp.path(), &l);
    let now = Utc::now();
    o.declare(def("c1", "bot:chief", None), now).await.unwrap();

    let first = o.check_later("c1", 5, None, now).await.unwrap();
    assert_eq!(first.clamped, Some("floor"));
    assert_eq!(first.wake.due_at, now + Duration::seconds(60));
    assert!(first.replaced.is_none());

    let second = o
        .check_later("c1", 999_999, Some("look again"), now)
        .await
        .unwrap();
    assert_eq!(second.clamped, Some("ceiling"));
    assert_eq!(second.wake.due_at, now + Duration::seconds(3600));
    assert_eq!(second.replaced.unwrap().wake_id, first.wake.wake_id);

    let c = o.get("c1").await.unwrap();
    let pending = c.pending_check.unwrap();
    assert_eq!(pending.wake_id, second.wake.wake_id);
    assert_eq!(pending.message, "look again");
    // Exactly one pending wake for the campaign key.
    let q = WakeQueue::new(l.clone());
    assert_eq!(
        q.pending()
            .await
            .unwrap()
            .keys()
            .filter(|k| k.starts_with("campaign:c1"))
            .count(),
        1
    );

    // Finishing stands the check down; a finished campaign cannot re-arm.
    o.finish("c1", None).await.unwrap();
    assert!(o.get("c1").await.unwrap().pending_check.is_none());
    assert!(o.check_later("c1", 120, None, now).await.is_err());
    // The view is derived and rebuildable.
    assert!(tmp
        .path()
        .join(".allternit/rails/campaigns/c1.json")
        .exists());
}

#[tokio::test]
async fn budget_exhaustion_pauses_and_raises_needs_you() {
    let tmp = test_root();
    let l = ledger(tmp.path());
    let o = ops(tmp.path(), &l);
    let now = Utc::now();
    let mut d = def("b1", "ao:claude", None);
    d.budget = Some(BudgetDecl {
        unit: "run".into(),
        limit: 3.0,
        mode: BudgetMode::Additive,
        per_wake: None,
    });
    o.declare(d, now).await.unwrap();
    let spend = |a: f64| SpendEntry {
        amount: a,
        start: None,
        end: None,
        resource: None,
        note: None,
    };
    let out = o.spend("b1", spend(2.0), now).await.unwrap();
    assert!(!out.paused_for_budget);
    let out = o.spend("b1", spend(1.0), now).await.unwrap();
    assert!(out.paused_for_budget);
    assert_eq!(out.campaign.status, CampaignStatus::Paused);
    assert_eq!(
        out.campaign.status_reason.as_deref(),
        Some("budget_exhausted")
    );

    let events = l.query(LedgerQuery::default()).await.unwrap();
    let open = open_needs_you(&events);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].key, "campaign:b1:budget");
    assert!(open[0].body.contains("do not cap provider bills"));

    // Resume refuses without a higher limit, accepts with one.
    assert!(o.resume("b1", None, now).await.is_err());
    assert!(o.resume("b1", Some(3.0), now).await.is_err());
    let c = o.resume("b1", Some(10.0), now).await.unwrap();
    assert_eq!(c.status, CampaignStatus::Active);
    assert_eq!(c.budget.unwrap().limit, 10.0);
}

#[tokio::test]
async fn shared_vs_additive_budget_on_a_campaign() {
    let tmp = test_root();
    let l = ledger(tmp.path());
    let o = ops(tmp.path(), &l);
    let now = Utc::now();
    for (id, mode) in [
        ("shared", BudgetMode::Shared),
        ("additive", BudgetMode::Additive),
    ] {
        let mut d = def(id, "bot:x", None);
        d.budget = Some(BudgetDecl {
            unit: "gpu-minute".into(),
            limit: 1000.0,
            mode,
            per_wake: None,
        });
        o.declare(d, now).await.unwrap();
        for _ in 0..2 {
            o.spend(
                id,
                SpendEntry {
                    amount: 60.0,
                    start: Some("2026-09-29T10:00:00Z".parse().unwrap()),
                    end: Some("2026-09-29T11:00:00Z".parse().unwrap()),
                    resource: Some("gpu0".into()),
                    note: None,
                },
                now,
            )
            .await
            .unwrap();
        }
    }
    let spent = |id: &str| {
        let o = &o;
        let id = id.to_string();
        async move { o.get(&id).await.unwrap().budget.unwrap().spent }
    };
    assert!((spent("shared").await - 60.0).abs() < 1e-9);
    assert!((spent("additive").await - 120.0).abs() < 1e-9);
}

#[tokio::test]
async fn executor_not_enabled_raises_needs_you_instead_of_running() {
    let tmp = test_root();
    let l = ledger(tmp.path());
    let o = ops(tmp.path(), &l);
    let now = Utc::now();
    let marker = tmp.path().join("ran.txt");
    let cmd = format!("echo ran >> '{}'", marker.display());
    o.declare(def("cmd", "command", Some(&cmd)), now)
        .await
        .unwrap();
    o.declare(def("bot", "bot:nightly-audit-engineer", None), now)
        .await
        .unwrap();
    o.check_later("cmd", 60, None, now).await.unwrap();
    o.check_later("bot", 60, None, now).await.unwrap();

    // Default config: nothing enabled. Even enabling `bot` does not spawn.
    let mut cfg = AutomationConfig::default();
    cfg.wake.enabled_executors = vec!["bot".into()];
    let report = run_due(&ctx(tmp.path(), &l, cfg, None), now + Duration::seconds(61))
        .await
        .unwrap();
    assert!(!report.locked);
    assert_eq!(report.fired.len(), 2);
    assert!(report.fired.iter().all(|f| f.outcome == "needs_you"));
    assert!(!marker.exists(), "command must not run when not enabled");

    let events = l.query(LedgerQuery::default()).await.unwrap();
    let keys: Vec<String> = open_needs_you(&events).into_iter().map(|i| i.key).collect();
    assert!(keys.contains(&"campaign:cmd:check".to_string()));
    assert!(keys.contains(&"campaign:bot:check".to_string()));
}

#[tokio::test]
async fn enabled_allowlisted_command_runs_rearms_and_meters() {
    let tmp = test_root();
    let l = ledger(tmp.path());
    let o = ops(tmp.path(), &l);
    let now = Utc::now();
    let marker = tmp.path().join("ran.txt");
    let cmd = format!("echo \"$ALLTERNIT_CAMPAIGN_ID\" >> '{}'", marker.display());
    let mut d = def("sweep", "command", Some(&cmd));
    d.budget = Some(BudgetDecl {
        unit: "sweep-run".into(),
        limit: 2.0,
        mode: BudgetMode::Additive,
        per_wake: Some(1.0),
    });
    d.rearm = Some(Rearm {
        every_secs: Some(600),
        weekdays: vec![],
        at: None,
        timezone: None,
    });
    let c = o.declare(d, now).await.unwrap();
    assert_eq!(
        c.pending_check.as_ref().unwrap().due_at,
        now + Duration::seconds(600)
    );

    let mut cfg = AutomationConfig::default();
    cfg.wake.enabled_executors = vec!["command".into()];
    cfg.wake.command_allowlist = vec![cmd.clone()];
    let t1 = now + Duration::seconds(601);
    let r = run_due(&ctx(tmp.path(), &l, cfg.clone(), None), t1)
        .await
        .unwrap();
    assert_eq!(r.fired.len(), 1);
    assert_eq!(r.fired[0].outcome, "ran");
    assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "sweep");
    let c = o.get("sweep").await.unwrap();
    assert_eq!(c.budget.as_ref().unwrap().spent, 1.0);
    assert_eq!(
        c.pending_check.as_ref().unwrap().due_at,
        t1 + Duration::seconds(600)
    );

    // Second run exhausts the budget: paused, no re-arm, needs-you raised.
    let t2 = t1 + Duration::seconds(601);
    let r = run_due(&ctx(tmp.path(), &l, cfg, None), t2).await.unwrap();
    assert_eq!(r.fired.len(), 1);
    let c = o.get("sweep").await.unwrap();
    assert_eq!(c.status, CampaignStatus::Paused);
    assert!(c.pending_check.is_none());
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_sweeps_fire_once() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    let l = ledger(&root);
    let o = ops(&root, &l);
    let now = Utc::now();
    let marker = root.join("ran.txt");
    // Slow command so the sweeps overlap.
    let cmd = format!("sleep 1; echo x >> '{}'", marker.display());
    o.declare(def("lock", "command", Some(&cmd)), now)
        .await
        .unwrap();
    o.check_later("lock", 60, None, now).await.unwrap();
    let mut cfg = AutomationConfig::default();
    cfg.wake.enabled_executors = vec!["command".into()];
    cfg.wake.command_allowlist = vec![cmd];
    let at = now + Duration::seconds(61);

    let mut handles = Vec::new();
    for _ in 0..2 {
        let (root, l, cfg) = (root.clone(), l.clone(), cfg.clone());
        handles.push(tokio::spawn(async move {
            run_due(&ctx(&root, &l, cfg, None), at).await.unwrap()
        }));
    }
    let mut reports = Vec::new();
    for h in handles {
        reports.push(h.await.unwrap());
    }
    let fired: usize = reports.iter().map(|r| r.fired.len()).sum();
    assert_eq!(fired, 1, "exactly one sweep fires the wake");
    assert_eq!(reports.iter().filter(|r| r.locked).count(), 1);
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);

    // A later sweep sees the wake already fired and does nothing.
    let again = run_due(&ctx(&root, &l, cfg, None), at).await.unwrap();
    assert!(!again.locked);
    assert!(again.fired.is_empty());
    assert_eq!(events_of(&l, "WakeFired").await.len(), 1);
    assert_eq!(events_of(&l, "WakeCompleted").await.len(), 1);
}

#[tokio::test]
async fn claimed_but_uncompleted_wake_is_not_refired() {
    let tmp = test_root();
    let l = ledger(tmp.path());
    let q = WakeQueue::new(l.clone());
    let (w, _) = q
        .schedule(
            "campaign:ghost",
            wake::WakeTarget::Campaign {
                campaign_id: "ghost".into(),
            },
            Utc::now() - Duration::seconds(1),
            "m",
            "t",
        )
        .await
        .unwrap();
    // Simulate a sweep that claimed the wake and died.
    l.append(allternit_commrails::AllternitEvent {
        event_id: String::new(),
        ts: String::new(),
        actor: allternit_commrails::Actor {
            r#type: allternit_commrails::ActorType::Gate,
            id: "wake".into(),
        },
        scope: None,
        r#type: "WakeFired".into(),
        payload: json!({"wake_id": w.wake_id, "key": w.key}),
        provenance: None,
    })
    .await
    .unwrap();
    let r = run_due(
        &ctx(tmp.path(), &l, AutomationConfig::default(), None),
        Utc::now(),
    )
    .await
    .unwrap();
    assert!(r.fired.is_empty());
    assert_eq!(r.unfinished.len(), 1);
    assert_eq!(r.unfinished[0].wake_id, w.wake_id);
}

#[tokio::test]
async fn check_wake_during_quiet_hours_is_queued_then_released() {
    let tmp = test_root();
    let l = ledger(tmp.path());
    let cfg_att = AttentionConfig {
        timezone: "America/Chicago".into(),
        quiet_hours: Some(QuietHoursConfig {
            start: "22:30".into(),
            end: "06:00".into(),
        }),
        ..Default::default()
    };
    let o = CampaignOps::new(tmp.path(), l.clone(), 3600, cfg_att.clone());
    // 23:00 CDT
    let night: DateTime<Utc> = "2026-09-30T04:00:00Z".parse().unwrap();
    o.declare(def("q", "bot:chief", None), night).await.unwrap();
    o.check_later("q", 60, None, night).await.unwrap();
    let cfg = AutomationConfig {
        attention: cfg_att,
        ..Default::default()
    };
    let r = run_due(
        &ctx(tmp.path(), &l, cfg.clone(), None),
        night + Duration::seconds(61),
    )
    .await
    .unwrap();
    assert_eq!(r.fired[0].outcome, "needs_you");
    let events = l.query(LedgerQuery::default()).await.unwrap();
    assert!(
        open_needs_you(&events).is_empty(),
        "queued, not delivered, in quiet hours"
    );
    // Next sweep after 06:00 CDT releases it.
    let morning: DateTime<Utc> = "2026-09-30T11:05:00Z".parse().unwrap();
    let r = run_due(&ctx(tmp.path(), &l, cfg, None), morning)
        .await
        .unwrap();
    assert_eq!(r.released.len(), 1);
    assert_eq!(r.released[0].decision, "delivered");
    let events = l.query(LedgerQuery::default()).await.unwrap();
    assert_eq!(open_needs_you(&events).len(), 1);
}

async fn build_gate(root: &Path) -> (Arc<Ledger>, Gate) {
    let l = ledger(root);
    let leases = Arc::new(
        Leases::new(LeasesOptions {
            root_dir: Some(root.to_path_buf()),
            leases_dir: Some(PathBuf::from(".allternit/leases")),
            event_sink: Some(l.clone()),
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
        ledger: l.clone(),
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
    (l, gate)
}

#[tokio::test]
async fn timer_wait_gate_registers_a_wake_and_sweep_flips_readiness() {
    let tmp = test_root();
    let (l, gate) = build_gate(tmp.path()).await;
    let (_, dag_id, root) = gate.plan_new("timers", None).await.unwrap();
    let node_id = "wk_timer_n".to_string();
    gate.plan_refine(
        &dag_id,
        "one node",
        "agent",
        vec![Mutation::CreateNode {
            node_id: node_id.clone(),
            node_kind: "task".into(),
            title: "N".into(),
            parent_node_id: Some(root),
            execution_mode: "shared".into(),
            description: None,
            executor: None,
        }],
    )
    .await
    .unwrap();
    let until = |s: &str| HashMap::from([("until".to_string(), json!(s))]);
    // Later timer first, then an elapsed one: the node key coalesces to the
    // newest wake (due now); firing resolves the elapsed gate and re-arms for
    // the later one.
    gate.add_node_wait_gate(
        &dag_id,
        &node_id,
        WaitGateKind::Timer,
        None,
        until("2999-01-01T00:00:00Z"),
        "t",
    )
    .await
    .unwrap();
    gate.add_node_wait_gate(
        &dag_id,
        &node_id,
        WaitGateKind::Timer,
        None,
        until("2020-01-01T00:00:00Z"),
        "t",
    )
    .await
    .unwrap();
    let key = wake::node_key(&dag_id, &node_id);
    let q = WakeQueue::new(l.clone());
    let pending = q.pending().await.unwrap();
    assert_eq!(
        pending[&key].due_at.to_rfc3339(),
        "2020-01-01T00:00:00+00:00"
    );
    assert_eq!(events_of(&l, "WakeScheduled").await.len(), 2);

    // Without a gate handler node wakes stay pending.
    let r = run_due(
        &ctx(tmp.path(), &l, AutomationConfig::default(), None),
        Utc::now(),
    )
    .await
    .unwrap();
    assert!(r.fired.is_empty());

    let handler: &dyn NodeTimerHandler = &gate;
    let r = run_due(
        &ctx(tmp.path(), &l, AutomationConfig::default(), Some(handler)),
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!(r.fired.len(), 1);
    assert_eq!(r.fired[0].outcome, "resolved_timers");
    let resolved = events_of(&l, "DagNodeWaitGateResolved").await;
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["reason"], json!("timer elapsed"));
    let pending = q.pending().await.unwrap();
    assert_eq!(
        pending[&key].due_at.to_rfc3339(),
        "2999-01-01T00:00:00+00:00"
    );
}
