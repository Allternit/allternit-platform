//! `drive` runner: end-to-end over stub harnesses (no paid agents).
//!
//! Stub harnesses are shell scripts. One is named `claude` so the spawn gate
//! classifies it as hooked (it receives, and ignores, the gate's
//! `--permission-mode bypassPermissions --settings <file>` args); one is named
//! `kimi` so it is ungated. Sessions run in real tmux, like production.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::drive::hooks::NoHooks;
use allternit_factory_engine::drive::{
    DriveExit, DriveOptions, DriveReport, Driver, ATTEMPT_FINISHED, ATTEMPT_STARTED, BOT_NOTIFIED,
    NEEDS_YOU, SPAWN_DEFERRED,
};
use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::templates::RETRY_SAFE_LABEL;
use allternit_factory_engine::wait_gates::GateOutcome;
use allternit_factory_engine::work::needs_you::pending_manual_gates;
use allternit_factory_engine::work::{project_dag, DagState};
use allternit_factory_engine::{
    Actor, ActorType, AllternitEvent, Gate, GateOptions, Leases, Ledger, LedgerQuery, ReceiptStore,
    ReceiptStoreOptions,
};
use serde_json::{json, Value};
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix("drive-").tempdir_in(base).unwrap()
}

fn commrails_bin() -> &'static str {
    env!("CARGO_BIN_EXE_allternit-factory")
}

async fn build_gate(root: &Path) -> (Arc<Ledger>, Arc<Gate>) {
    // The spawn gate installs its hook with this binary.
    std::env::set_var("ALLTERNIT_COMMRAILS_BIN", commrails_bin());
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

/// Write a stub harness `<root>/stubs/<name>`. `body` runs with `$1` = the
/// prompt file and `$2` = the node id; `$LOG` is the shared run log.
fn stub(root: &Path, name: &str, body: &str) -> PathBuf {
    let dir = root.join("stubs");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let log = root.join("stub.log");
    std::fs::write(
        &path,
        format!("#!/bin/sh\nLOG='{}'\nprompt_file=\"$1\"\nnode=\"$2\"\n{body}\n", log.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// A stub that logs start/end (epoch millis via perl), sleeps, and prints
/// its node id plus the whole prompt on stdout.
const WORKER: &str = r#"now() { perl -MTime::HiRes=time -e 'printf "%d\n", time*1000'; }
echo "start $node $(now)" >> "$LOG"
sleep "${SLEEP:-0}"
echo "end $node $(now)" >> "$LOG"
echo "output of $node"
cat "$prompt_file""#;

fn worker(root: &Path, sleep_s: &str) -> PathBuf {
    stub(root, "claude", &format!("SLEEP={sleep_s}\n{WORKER}"))
}

fn write_config(root: &Path, extra: Value) {
    let claude = root.join("stubs/claude");
    let kimi = root.join("stubs/kimi");
    let mut cfg = json!({
        "poll_interval_ms": 100,
        "min_free_mem_mb": 0,
        "max_load_per_cpu": 100000.0,
        "harnesses": {
            "claude": { "argv": [claude, "{prompt_file}", "{node_id}"] },
            "kimi": { "argv": [kimi, "{prompt_file}", "{node_id}"] }
        }
    });
    for (k, v) in extra.as_object().unwrap() {
        cfg[k] = v.clone();
    }
    let dir = root.join(".allternit/drive");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
}

fn node(id: &str, parent: &str, executor: Option<&str>, description: Option<&str>) -> Mutation {
    Mutation::CreateNode {
        node_id: id.to_string(),
        node_kind: "task".to_string(),
        title: id.to_uppercase(),
        parent_node_id: Some(parent.to_string()),
        execution_mode: "shared".to_string(),
        description: description.map(String::from),
        executor: executor.map(String::from),
    }
}

fn blocked_by(blocker: &str, blocked: &str) -> Mutation {
    Mutation::AddBlockedBy { from_node_id: blocker.into(), to_node_id: blocked.into() }
}

async fn plan(gate: &Gate, mutations: Vec<Mutation>) -> (String, String) {
    let (_, dag_id, root) = gate.plan_new("drive test", None).await.unwrap();
    let mutations = mutations
        .into_iter()
        .map(|m| match m {
            Mutation::CreateNode { node_id, node_kind, title, execution_mode, description, executor, .. } => {
                Mutation::CreateNode {
                    node_id,
                    node_kind,
                    title,
                    parent_node_id: Some(root.clone()),
                    execution_mode,
                    description,
                    executor,
                }
            }
            other => other,
        })
        .collect();
    gate.plan_refine(&dag_id, "drive test nodes", "test", mutations).await.unwrap();
    (dag_id, root)
}

async fn dag(ledger: &Ledger, dag_id: &str) -> DagState {
    let events: Vec<AllternitEvent> = ledger
        .query(LedgerQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .collect();
    project_dag(&events, dag_id)
}

async fn events_of(ledger: &Ledger, ty: &str) -> Vec<AllternitEvent> {
    ledger
        .query(LedgerQuery { r#type: Some(ty.to_string()), ..Default::default() })
        .await
        .unwrap()
}

async fn drive(root: &Path, ledger: &Arc<Ledger>, gate: &Arc<Gate>, dag_id: &str, f: impl FnOnce(&mut DriveOptions)) -> DriveReport {
    let mut opts = DriveOptions { dag_id: dag_id.to_string(), ..Default::default() };
    f(&mut opts);
    let gate = (!opts.dry_run).then(|| gate.clone());
    let mut driver = Driver::new(root.to_path_buf(), ledger.clone(), gate, opts, Arc::new(NoHooks)).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        driver.run(std::future::pending::<()>()),
    )
    .await
    .expect("drive did not finish")
    .unwrap()
}

fn read_output(root: &Path, dag: &DagState, node_id: &str) -> String {
    let out = dag.nodes[node_id].output.as_ref().expect("node output");
    std::fs::read_to_string(root.join(&out.output_path)).unwrap()
}

/// `(node, start_ms, end_ms)` from the stub log.
fn intervals(root: &Path) -> Vec<(String, u128, u128)> {
    let text = std::fs::read_to_string(root.join("stub.log")).unwrap_or_default();
    let mut starts: HashMap<String, u128> = HashMap::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let (kind, node, ms) = (parts[0], parts[1].to_string(), parts[2].parse::<u128>().unwrap());
        if kind == "start" {
            starts.insert(node, ms);
        } else {
            out.push((node.clone(), starts[&node], ms));
        }
    }
    out
}

fn max_overlap(iv: &[(String, u128, u128)]) -> usize {
    iv.iter()
        .map(|(_, s, _)| iv.iter().filter(|(_, s2, e2)| s2 <= s && s < e2).count())
        .max()
        .unwrap_or(0)
}

#[tokio::test]
async fn three_node_chain_runs_end_to_end_with_outputs() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, plan_root) = plan(
        &gate,
        vec![
            node("e2e_a", "", Some("ao:claude"), Some("first step")),
            node("e2e_b", "", Some("ao:claude"), Some("second, from: {{ e2e_a.output }}")),
            node("e2e_c", "", Some("ao:claude"), Some("third, from: {{ e2e_b.output }}")),
            blocked_by("e2e_a", "e2e_b"),
            blocked_by("e2e_b", "e2e_c"),
        ],
    )
    .await;

    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert_eq!(report.exit, Some(DriveExit::Idle));
    let spawned: Vec<&str> = report.spawned.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(spawned, vec!["e2e_a", "e2e_b", "e2e_c"]);

    let d = dag(&ledger, &dag_id).await;
    for n in ["e2e_a", "e2e_b", "e2e_c"] {
        assert_eq!(d.nodes[n].status, "DONE", "{n}");
        assert!(d.nodes[n].current_wih_id.is_none());
    }
    // c's prompt was resolved from b's output, which carried a's.
    let c_out = read_output(&root, &d, "e2e_c");
    assert!(c_out.starts_with("output of e2e_c"), "{c_out}");
    // Upstream outputs arrive nonce-fenced (#968 S7): assert the chain through
    // the <untrusted-data> wrappers rather than raw concatenation.
    assert!(c_out.contains("third, from: <untrusted-data nonce="), "{c_out}");
    assert!(c_out.contains("source=\"node:e2e_b\">\noutput of e2e_b"), "{c_out}");
    assert!(c_out.contains("second, from: &lt;untrusted-data nonce="), "{c_out}");
    assert!(c_out.contains("output of e2e_a"), "{c_out}");

    // Every attempt started and finished `done`; WIHs were open-signed.
    assert_eq!(events_of(&ledger, ATTEMPT_STARTED).await.len(), 3);
    let finished = events_of(&ledger, ATTEMPT_FINISHED).await;
    assert!(finished.iter().all(|e| e.payload["outcome"] == "done" && e.payload["receipt_id"].is_string()));
    assert_eq!(events_of(&ledger, "WIHOpenSigned").await.len(), 3);
    // The plan root is left for the operator to verify and close.
    assert!(report.waiting.iter().any(|l| l.contains(&plan_root) && l.contains("verify and close")));
    // Sessions were cleaned up and caps released.
    let caps: Value = serde_json::from_str(&std::fs::read_to_string(root.join(".allternit/drive/caps.json")).unwrap()).unwrap();
    assert_eq!(caps["running"].as_array().unwrap().len(), 0);
    assert_eq!(caps["spawns"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn per_dag_concurrency_cap_holds() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0.6");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(
        &gate,
        vec![
            node("pc_a", "", Some("ao:claude"), None),
            node("pc_b", "", Some("ao:claude"), None),
            node("pc_c", "", Some("ao:claude"), None),
        ],
    )
    .await;
    let report = drive(&root, &ledger, &gate, &dag_id, |o| o.max_concurrent = Some(1)).await;
    assert_eq!(report.spawned.len(), 3);
    let iv = intervals(&root);
    assert_eq!(iv.len(), 3);
    assert_eq!(max_overlap(&iv), 1, "{iv:?}");
    let deferred = events_of(&ledger, SPAWN_DEFERRED).await;
    assert!(deferred.iter().any(|e| e.payload["reason"] == "max_concurrent"));
}

/// Two `drive` processes on two DAGs share the global caps under `.allternit/drive`.
#[tokio::test]
async fn global_caps_are_shared_across_two_drive_processes() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0.8");
    write_config(&root, json!({ "global_max_concurrent": 1, "global_max_spawns_per_hour": 4 }));
    let (ledger, gate) = build_gate(&root).await;
    let (dag1, _) = plan(&gate, vec![node("g1_a", "", Some("ao:claude"), None), node("g1_b", "", Some("ao:claude"), None)]).await;
    let (dag2, _) = plan(&gate, vec![node("g2_a", "", Some("ao:claude"), None), node("g2_b", "", Some("ao:claude"), None)]).await;

    let run = |dag: String| {
        let root = root.clone();
        tokio::process::Command::new(commrails_bin())
            .env("ALLTERNIT_COMMRAILS_BIN", commrails_bin())
            .args(["internal", "rails", "--root", root.to_str().unwrap(), "drive", &dag])
            .output()
    };
    let (o1, o2) = tokio::join!(run(dag1.clone()), run(dag2.clone()));
    let (o1, o2) = (o1.unwrap(), o2.unwrap());
    assert!(o1.status.success(), "{}", String::from_utf8_lossy(&o1.stderr));
    assert!(o2.status.success(), "{}", String::from_utf8_lossy(&o2.stderr));

    let iv = intervals(&root);
    assert_eq!(iv.len(), 4, "{iv:?}");
    assert_eq!(max_overlap(&iv), 1, "global concurrency cap crossed: {iv:?}");
    for d in [&dag1, &dag2] {
        let s = dag(&ledger, d).await;
        assert!(s.nodes.values().filter(|n| n.parent_node_id.is_some()).all(|n| n.status == "DONE"));
    }
    assert!(events_of(&ledger, SPAWN_DEFERRED)
        .await
        .iter()
        .any(|e| e.payload["reason"] == "global_max_concurrent"));

    // Hourly: 4 of 4 global spawns used. A third DAG is deferred, not spawned.
    let (dag3, _) = plan(&gate, vec![node("g3_a", "", Some("ao:claude"), None)]).await;
    let out = tokio::process::Command::new(commrails_bin())
        .args(["internal", "rails", "--root", root.to_str().unwrap(), "drive", &dag3, "--once"])
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("spawn deferred: global_max_spawns_per_hour (4/4)"), "{stdout}");
    let deferred: Vec<_> = events_of(&ledger, SPAWN_DEFERRED)
        .await
        .into_iter()
        .filter(|e| e.payload["dag_id"] == json!(dag3))
        .collect();
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0].payload["reason"], "global_max_spawns_per_hour");
    assert!(dag(&ledger, &dag3).await.nodes["g3_a"].current_wih_id.is_none(), "no pickup when deferred");

    // A second process on the same DAG is refused by the per-DAG lock.
    let lock = allternit_factory_engine::drive::caps::FileLock::try_exclusive(
        &root.join(format!(".allternit/drive/dags/{dag3}.lock")),
    )
    .unwrap()
    .unwrap();
    let out = tokio::process::Command::new(commrails_bin())
        .args(["internal", "rails", "--root", root.to_str().unwrap(), "drive", &dag3, "--once"])
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already driving"));
    drop(lock);
}

#[tokio::test]
async fn ungated_harness_is_admitted_and_spawned() {
    // Eoj, 2026-09-30: ungated CLIs run in their own auto-approve mode;
    // Allternit's gate is the gate, so drive never refuses them.
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    stub(&root, "kimi", "echo ran >> \"$LOG\"");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(&gate, vec![node("ug_a", "", Some("ao:kimi"), None)]).await;

    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert!(report.needs_you.is_empty(), "{:?}", report.needs_you);
    assert!(report.spawned.iter().any(|(n, _)| n == "ug_a"), "{:?}", report.spawned);
    assert!(events_of(&ledger, NEEDS_YOU).await.is_empty());
}

#[tokio::test]
async fn bot_executor_is_mailed_not_spawned() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(&gate, vec![node("bot_a", "", Some("bot:scout"), None)]).await;
    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert!(report.spawned.is_empty());
    assert_eq!(report.bot_notified, vec!["bot_a".to_string()]);
    let sent = events_of(&ledger, "MessageSent").await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].payload["thread_id"], json!(format!("dag:{dag_id}")));
    assert_eq!(sent[0].payload["to_agents"], json!(["bot:scout"]));
    // Idempotent across runs; the bot can still pick it up (no gate added).
    drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert_eq!(events_of(&ledger, BOT_NOTIFIED).await.len(), 1);
    assert_eq!(events_of(&ledger, "MessageSent").await.len(), 1);
    assert!(dag(&ledger, &dag_id).await.nodes["bot_a"].wait_gates.is_empty());
    gate.wih_pickup(&dag_id, "bot_a", "bot:scout").await.unwrap();
}

/// Simulate a drive process that died after spawning: an open attempt whose
/// session is gone and which wrote no exit code.
async fn interrupted_attempt(ledger: &Ledger, gate: &Gate, root: &Path, dag_id: &str, node_id: &str) -> (String, String) {
    let wih = gate.wih_pickup(dag_id, node_id, "drive-claude").await.unwrap();
    let attempt_id = format!("{wih}-a1");
    ledger
        .append(AllternitEvent {
            event_id: String::new(),
            ts: String::new(),
            actor: Actor { r#type: ActorType::Agent, id: "drive".into() },
            scope: None,
            r#type: ATTEMPT_STARTED.to_string(),
            payload: json!({
                "dag_id": dag_id, "node_id": node_id, "wih_id": wih, "attempt_id": attempt_id,
                "attempt": 1, "executor": "ao:claude", "harness": "claude",
                "slug": format!("drive-gone-{node_id}"),
                "run_dir": root.join(".allternit/drive/runs/gone"), "timeout_seconds": 60
            }),
            provenance: None,
        })
        .await
        .unwrap();
    (wih, attempt_id)
}

#[tokio::test]
async fn interrupted_non_idempotent_node_is_not_restarted() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(&gate, vec![node("int_a", "", Some("ao:claude"), None)]).await;
    let (wih, attempt_id) = interrupted_attempt(&ledger, &gate, &root, &dag_id, "int_a").await;

    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert!(report.spawned.is_empty(), "must not auto-restart");
    assert_eq!(report.needs_you, vec![("int_a".to_string(), "interrupted".to_string())]);
    let finished = events_of(&ledger, ATTEMPT_FINISHED).await;
    assert_eq!(finished[0].payload["outcome"], "interrupted");
    let d = dag(&ledger, &dag_id).await;
    let g = &d.nodes["int_a"].wait_gates[0];
    assert_eq!(g.params["attempt_id"], json!(attempt_id));
    assert!(!root.join("stub.log").exists());

    // A human checks the effects and resolves: drive restarts on the same WIH.
    gate.resolve_node_wait_gate(&dag_id, "int_a", &g.gate_id, GateOutcome::Ok, Some(Actor { r#type: ActorType::User, id: "eoj".into() }), None)
        .await
        .unwrap();
    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert_eq!(report.spawned.len(), 1);
    let started = events_of(&ledger, ATTEMPT_STARTED).await;
    assert_eq!(started[1].payload["wih_id"], json!(wih));
    assert_eq!(started[1].payload["restart_of"], json!(attempt_id));
    assert_eq!(dag(&ledger, &dag_id).await.nodes["int_a"].status, "DONE");
}

#[tokio::test]
async fn interrupted_retry_safe_node_restarts() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(
        &gate,
        vec![
            node("safe_a", "", Some("ao:claude"), None),
            Mutation::AddLabel { node_id: "safe_a".into(), label: RETRY_SAFE_LABEL.into() },
        ],
    )
    .await;
    interrupted_attempt(&ledger, &gate, &root, &dag_id, "safe_a").await;
    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert!(report.needs_you.is_empty());
    assert_eq!(report.spawned.len(), 1);
    assert_eq!(dag(&ledger, &dag_id).await.nodes["safe_a"].status, "DONE");
}

#[tokio::test]
async fn dead_and_timed_out_sessions_fail_with_receipts() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    // claude stub: dead_a kills its runner shell (no exit code); slow_a hangs.
    stub(
        &root,
        "claude",
        "echo partial\ncase \"$node\" in dead_a) sleep 1; kill -9 $PPID ;; slow_a) sleep 30 ;; esac",
    );
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(
        &gate,
        vec![
            node("dead_a", "", Some("ao:claude"), None),
            node("slow_a", "", Some("ao:claude"), None),
            node("after_dead", "", Some("ao:claude"), None),
            blocked_by("dead_a", "after_dead"),
        ],
    )
    .await;
    let report = drive(&root, &ledger, &gate, &dag_id, |o| o.timeout_seconds = Some(2)).await;
    let mut finished = report.finished.clone();
    finished.sort();
    assert_eq!(finished, vec![("dead_a".into(), "dead".into()), ("slow_a".into(), "timeout".into())]);
    let d = dag(&ledger, &dag_id).await;
    for n in ["dead_a", "slow_a"] {
        assert_eq!(d.nodes[n].status, "FAILED", "{n}");
        let out = read_output(&root, &d, n);
        assert!(out.contains("drive attempt"), "{out}");
    }
    assert!(read_output(&root, &d, "slow_a").contains("timed out"));
    // No silent retry, and the dependent never ran.
    assert_eq!(events_of(&ledger, ATTEMPT_STARTED).await.len(), 2);
    assert_eq!(d.nodes["after_dead"].status, "NEW");
}

#[tokio::test]
async fn dry_run_has_no_side_effects() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    stub(&root, "kimi", "true");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(
        &gate,
        vec![
            node("dr_a", "", Some("ao:claude"), None),
            node("dr_b", "", Some("ao:kimi"), None),
            node("dr_c", "", Some("bot:scout"), None),
            node("dr_d", "", Some("ao:claude"), None),
            blocked_by("dr_a", "dr_d"),
        ],
    )
    .await;
    let before = ledger.query(LedgerQuery::default()).await.unwrap().len();
    let report = drive(&root, &ledger, &gate, &dag_id, |o| o.dry_run = true).await;
    assert_eq!(report.exit, Some(DriveExit::DryRun));
    assert_eq!(ledger.query(LedgerQuery::default()).await.unwrap().len(), before);
    assert!(!root.join(".allternit/drive/caps.json").exists());
    assert!(!root.join(".allternit/drive/runs").exists());
    assert!(!root.join("stub.log").exists());
    let plan = report.plan.join("\n");
    assert!(plan.contains("dr_a: would wih pickup + sign-open, spawn ao:claude"), "{plan}");
    assert!(plan.contains("--permission-mode bypassPermissions --settings"), "{plan}");
    assert!(plan.contains("dr_b: would wih pickup + sign-open, spawn ao:kimi"), "{plan}");
    assert!(plan.contains("dr_c: would mail bot:scout"), "{plan}");
    assert!(!plan.contains("dr_d"), "blocked node is not planned: {plan}");
}

#[tokio::test]
async fn capacity_admission_refuses_to_start() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    write_config(&root, json!({ "min_free_mem_mb": u64::MAX / 2 }));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(&gate, vec![node("cap_a", "", Some("ao:claude"), None)]).await;
    let mut driver = Driver::new(
        root.clone(),
        ledger.clone(),
        Some(gate.clone()),
        DriveOptions { dag_id: dag_id.clone(), ..Default::default() },
        Arc::new(NoHooks),
    )
    .unwrap();
    let err = driver.run(std::future::pending::<()>()).await.unwrap_err();
    assert!(format!("{err:#}").contains("capacity admission refused"), "{err:#}");
    assert_eq!(events_of(&ledger, "DriveCapacityRefused").await.len(), 1);
    assert!(dag(&ledger, &dag_id).await.nodes["cap_a"].current_wih_id.is_none());
}

#[tokio::test]
async fn template_retry_safe_becomes_a_node_label() {
    use allternit_factory_engine::templates::{parse_markdown_template, plan_from_template};
    let tmp = test_root();
    let (ledger, gate) = build_gate(tmp.path()).await;
    let tpl = |retry: &str| {
        format!(
            "---\nname: r\ndescription: r\n---\n\n```yaml template-spec\nsteps:\n  - id: fetch\n    title: Fetch\n    executor: \"ao:claude\"\n    retry: {retry}\n  - id: post\n    title: Post\n    executor: \"ao:claude\"\n    blocked_by: [fetch]\n```\n"
        )
    };
    let template = parse_markdown_template("r", &tpl("safe")).unwrap();
    let result = plan_from_template(&gate, &template, &HashMap::new(), None, None).await.unwrap();
    let d = dag(&ledger, &result.dag_id).await;
    assert_eq!(d.nodes[&result.nodes["fetch"]].labels, vec![RETRY_SAFE_LABEL.to_string()]);
    assert!(d.nodes[&result.nodes["post"]].labels.is_empty());
    let rejected = match parse_markdown_template("r", &tpl("always")) {
        Err(_) => true,
        Ok(t) => plan_from_template(&gate, &t, &HashMap::new(), None, None).await.is_err(),
    };
    assert!(rejected, "retry must be `safe` or omitted");
}

/// A refused `wih close` goes to needs-you; after a human resolves it, drive
/// retries only the close from the captured output (the harness is not re-run).
#[tokio::test]
async fn refused_close_is_retried_from_captured_output_without_rerun() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    worker(&root, "0");
    write_config(&root, json!({}));
    let (ledger, gate) = build_gate(&root).await;
    let (dag_id, _) = plan(&gate, vec![node("cf_a", "", Some("ao:claude"), None)]).await;
    let wih = gate.wih_pickup(&dag_id, "cf_a", "drive-claude").await.unwrap();
    let attempt_id = format!("{wih}-a1");
    let run_dir = root.join(".allternit/drive/runs/cf");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("stdout.txt"), "collected output\n").unwrap();
    std::fs::write(run_dir.join("exit_code"), "0\n").unwrap();
    for (ty, extra) in [
        (ATTEMPT_STARTED, json!({ "slug": "drive-cf-gone", "run_dir": run_dir, "executor": "ao:claude", "timeout_seconds": 60 })),
        (ATTEMPT_FINISHED, json!({ "outcome": "close_failed", "reason": "simulated Gate 4 refusal" })),
    ] {
        let mut payload = json!({ "dag_id": dag_id, "node_id": "cf_a", "wih_id": wih, "attempt_id": attempt_id });
        for (k, v) in extra.as_object().unwrap() {
            payload[k] = v.clone();
        }
        ledger
            .append(AllternitEvent {
                event_id: String::new(),
                ts: String::new(),
                actor: Actor { r#type: ActorType::Agent, id: "drive".into() },
                scope: None,
                r#type: ty.to_string(),
                payload,
                provenance: None,
            })
            .await
            .unwrap();
    }

    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert_eq!(report.needs_you, vec![("cf_a".to_string(), "attempt_failed".to_string())]);
    let d = dag(&ledger, &dag_id).await;
    let g = d.nodes["cf_a"].wait_gates[0].clone();
    assert_eq!(g.params["attempt_id"], json!(attempt_id));

    gate.resolve_node_wait_gate(&dag_id, "cf_a", &g.gate_id, GateOutcome::Ok, Some(Actor { r#type: ActorType::User, id: "eoj".into() }), None)
        .await
        .unwrap();
    let report = drive(&root, &ledger, &gate, &dag_id, |_| {}).await;
    assert!(report.spawned.is_empty(), "harness must not be re-run");
    assert_eq!(report.finished, vec![("cf_a".to_string(), "done".to_string())]);
    let d = dag(&ledger, &dag_id).await;
    assert_eq!(d.nodes["cf_a"].status, "DONE");
    assert_eq!(read_output(&root, &d, "cf_a"), "collected output\n");
    assert!(!root.join("stub.log").exists());
}
