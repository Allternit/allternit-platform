//! Node folders, `proof add`, the campaign board and the node page (F6E).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::gate::gate::DagMutation as Mutation;
use allternit_factory_engine::leases::leases::LeasesOptions;
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::workspace::board::{self, CardStatus};
use allternit_factory_engine::workspace::{node_folder, node_page, proof};
use allternit_factory_engine::{
    Actor, ActorType, AllternitEvent, Gate, GateOptions, Leases, Ledger, LedgerQuery,
    ReceiptStore, ReceiptStoreOptions,
};
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn test_root() -> TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new().prefix("allternit-nodefolders-").tempdir_in(base).unwrap()
}

async fn build_gate(root: &Path) -> (Arc<Ledger>, Arc<ReceiptStore>, Gate) {
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
            root_dir: Some(root.to_path_buf()),
            receipts_dir: Some(PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(PathBuf::from(".allternit/blobs")),
        })
        .unwrap(),
    );
    let gate = Gate::new(GateOptions {
        ledger: ledger.clone(),
        leases,
        receipts: receipts.clone(),
        index: None,
        vault: None,
        oauth_vault: None,
        root_dir: Some(root.to_path_buf()),
        actor_id: Some("gate".to_string()),
        strict_provenance: None,
        visual_provider: None,
        visual_config: None,
    });
    (ledger, receipts, gate)
}

fn node(id: &str, parent: &str, description: Option<&str>) -> Mutation {
    Mutation::CreateNode {
        node_id: id.to_string(),
        node_kind: "task".to_string(),
        title: format!("Title {id}"),
        parent_node_id: Some(parent.to_string()),
        execution_mode: "shared".to_string(),
        description: description.map(str::to_string),
        executor: None,
    }
}

fn blocked_by(blocker: &str, blocked: &str) -> Mutation {
    Mutation::AddBlockedBy { from_node_id: blocker.to_string(), to_node_id: blocked.to_string() }
}

fn folder(root: &Path, dag: &str, node: &str) -> PathBuf {
    root.join(node_folder::node_folder_rel_path(dag, node))
}

fn headings(md: &str) -> Vec<String> {
    md.lines().filter(|l| l.starts_with('#')).map(str::to_string).collect()
}

const STRUCTURED: &str = "Ship the export button.\n\n## Mini-requirements\n- CSV only\n- Max 10k rows\n\n## Proof contract\n- [ ] Export test passes — checked by: cargo test export\n- [ ] Screenshot of the button\n";

async fn all_events(ledger: &Ledger) -> Vec<AllternitEvent> {
    ledger.query(LedgerQuery::default()).await.unwrap()
}

#[tokio::test]
async fn plan_new_and_refine_write_node_folders_with_exact_headings() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    let (_ledger, _r, gate) = build_gate(&root).await;
    let raw = "Build the reporting export for the ops team";
    let (_, dag_id, root_node) = gate.plan_new(raw, None).await.unwrap();

    let dir = folder(&root, &dag_id, &root_node);
    let spec = std::fs::read_to_string(dir.join("SPEC.md")).unwrap();
    assert_eq!(
        headings(&spec),
        vec![format!("# {raw}"), "## Intent".into(), "## Mini-requirements".into(), "## Proof contract".into()]
    );
    assert!(spec.contains(&format!("## Intent\n{raw}\n")), "{spec}");
    assert!(spec.contains("## Mini-requirements\n- (none yet)\n"));
    assert!(spec.ends_with("## Proof contract\n- (none yet)\n"));
    assert!(dir.join("PROGRESS.md").is_file());
    assert!(dir.join("PROOF.md").is_file());
    assert!(dir.join("proof").is_dir());
    assert!(std::fs::read_to_string(dir.join("PROGRESS.md")).unwrap().starts_with(&format!("# Progress — {raw}")));
    assert!(std::fs::read_to_string(dir.join("PROOF.md")).unwrap().starts_with(&format!("# Proof — {raw}")));

    gate.plan_refine(
        &dag_id,
        "split",
        "agent",
        vec![node("nf_a", &root_node, Some(STRUCTURED)), node("nf_b", &root_node, None)],
    )
    .await
    .unwrap();
    let a = std::fs::read_to_string(folder(&root, &dag_id, "nf_a").join("SPEC.md")).unwrap();
    assert_eq!(
        headings(&a),
        vec!["# Title nf_a", "## Intent", "## Mini-requirements", "## Proof contract"]
    );
    assert!(a.contains("## Intent\nShip the export button.\n"));
    assert!(a.contains("1. CSV only\n2. Max 10k rows\n"));
    assert!(a.contains("- [ ] Export test passes — checked by: cargo test export\n- [ ] Screenshot of the button\n"));
    let b = std::fs::read_to_string(folder(&root, &dag_id, "nf_b").join("SPEC.md")).unwrap();
    assert!(b.contains("## Intent\nTitle nf_b\n"));
    // The derived output view path beside the folder stays free.
    assert!(!root.join(format!(".allternit/work/dags/{dag_id}/nodes/nf_a.out.md")).is_dir());

    let parsed = node_folder::read_spec(&root, &dag_id, "nf_a").unwrap();
    assert_eq!(parsed.mini_requirements, vec!["CSV only", "Max 10k rows"]);
    assert_eq!(parsed.proof_contract.len(), 2);
    assert_eq!(parsed.proof_contract[0].line, "Export test passes");
    assert_eq!(parsed.proof_contract[0].checked_by.as_deref(), Some("cargo test export"));
}

#[tokio::test]
async fn folders_are_idempotent_and_never_overwrite_edits() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    let (_ledger, _r, gate) = build_gate(&root).await;
    let (_, dag_id, root_node) = gate.plan_new("idempotent", None).await.unwrap();
    gate.plan_refine(&dag_id, "one", "agent", vec![node("id_a", &root_node, Some(STRUCTURED))])
        .await
        .unwrap();
    let spec_path = folder(&root, &dag_id, "id_a").join("SPEC.md");
    let edited = "# Mine\n\n## Intent\nA person rewrote this.\n\n## Mini-requirements\n1. x\n\n## Proof contract\n- [ ] y\n";
    std::fs::write(&spec_path, edited).unwrap();
    std::fs::remove_file(folder(&root, &dag_id, "id_a").join("PROGRESS.md")).unwrap();

    // Another mutation re-runs the hook for the whole DAG.
    gate.plan_refine(&dag_id, "two", "agent", vec![node("id_b", &root_node, None)])
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&spec_path).unwrap(), edited);
    // A missing file is recreated; the edited one is not.
    assert!(folder(&root, &dag_id, "id_a").join("PROGRESS.md").is_file());
    assert!(folder(&root, &dag_id, "id_b").join("SPEC.md").is_file());
}

#[tokio::test]
async fn proof_add_copies_file_writes_receipt_and_proof_entry() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    let (ledger, receipts, gate) = build_gate(&root).await;
    let (_, dag_id, root_node) = gate.plan_new("proof", None).await.unwrap();
    gate.plan_refine(&dag_id, "one", "agent", vec![node("pa_a", &root_node, Some(STRUCTURED))])
        .await
        .unwrap();

    let src = root.join("test-output.txt");
    std::fs::write(&src, b"42 passed").unwrap();
    let added = proof::add(&gate, &dag_id, "pa_a", "Export test passes", &src).await.unwrap();

    let hex = hex::encode(Sha256::digest(b"42 passed"));
    assert_eq!(added.sha256, format!("sha256:{hex}"));
    assert!(added.path.starts_with(&node_folder::node_folder_rel_path(&dag_id, "pa_a")));
    assert_eq!(std::fs::read(root.join(&added.path)).unwrap(), b"42 passed");

    let rcpt = receipts.read_receipt(&added.receipt_id).unwrap().expect("receipt on disk");
    assert_eq!(rcpt.tool, "proof.add");
    assert_eq!(rcpt.inputs_ref.as_deref(), Some(added.sha256.as_str()));
    let events = all_events(&ledger).await;
    let ev = events
        .iter()
        .find(|e| e.r#type == "ReceiptWritten" && e.payload["receipt_id"] == json!(added.receipt_id))
        .expect("ReceiptWritten on the ledger");
    assert_eq!(ev.payload["payload"]["sha256"], json!(added.sha256));
    assert_eq!(ev.payload["payload"]["line"], json!("Export test passes"));

    let proof_md = std::fs::read_to_string(folder(&root, &dag_id, "pa_a").join("PROOF.md")).unwrap();
    assert!(proof_md.contains(&format!("### Export test passes\n- proof/{}-test-output.txt · receipt {}", &hex[..12], added.receipt_id)), "{proof_md}");

    // Same bytes again: same stored name, a second receipt, a second entry.
    let again = proof::add(&gate, &dag_id, "pa_a", "1", &src).await.unwrap();
    assert_eq!(again.path, added.path);
    assert_ne!(again.receipt_id, added.receipt_id);
    let proof_md = std::fs::read_to_string(folder(&root, &dag_id, "pa_a").join("PROOF.md")).unwrap();
    assert_eq!(proof_md.matches("### Export test passes").count(), 1);
    assert_eq!(proof_md.matches("test-output.txt · receipt").count(), 2);

    // Refusals: unknown node, missing file, unknown contract line.
    assert!(proof::add(&gate, &dag_id, "ghost", "1", &src).await.is_err());
    assert!(proof::add(&gate, &dag_id, "pa_a", "1", &root.join("nope.txt")).await.is_err());
    assert!(proof::add(&gate, &dag_id, "pa_a", "Not a line", &src).await.is_err());
}

fn verdict_event(dag_id: &str, node_id: &str, outcome: &str) -> AllternitEvent {
    AllternitEvent {
        event_id: format!("evt_{}", rand_id()),
        ts: Utc::now().to_rfc3339(),
        actor: Actor { r#type: ActorType::Gate, id: "judge".into() },
        scope: None,
        r#type: "JudgeVerdictRecorded".into(),
        payload: json!({ "dag_id": dag_id, "node_id": node_id, "outcome": outcome, "reason": "t" }),
        provenance: None,
    }
}

fn rand_id() -> String {
    format!("{:x}", Sha256::digest(Utc::now().timestamp_nanos_opt().unwrap().to_string().as_bytes()))[..12].to_string()
}

#[tokio::test]
async fn board_waves_by_depth_and_counts_only_judge_and_receipts() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    let (ledger, _r, gate) = build_gate(&root).await;
    let (_, dag_id, root_node) = gate.plan_new("board plan", None).await.unwrap();
    gate.plan_refine(
        &dag_id,
        "chain",
        "agent",
        vec![
            node("bd_a", &root_node, Some(STRUCTURED)),
            node("bd_b", &root_node, None),
            node("bd_c", &root_node, None),
            node("bd_d", &root_node, None),
            blocked_by("bd_a", "bd_b"),
            blocked_by("bd_b", "bd_c"),
            blocked_by("bd_a", "bd_c"),
        ],
    )
    .await
    .unwrap();

    // A ticked checkbox in PROOF.md and SPEC.md must not count.
    let fa = folder(&root, &dag_id, "bd_a");
    let spec = std::fs::read_to_string(fa.join("SPEC.md")).unwrap().replace("- [ ] Export", "- [x] Export");
    std::fs::write(fa.join("SPEC.md"), spec).unwrap();
    let mut pmd = std::fs::read_to_string(fa.join("PROOF.md")).unwrap();
    pmd.push_str("\n### Export test passes\n- [x] proof/claimed.txt · done!\n");
    std::fs::write(fa.join("PROOF.md"), pmd).unwrap();

    let events = all_events(&ledger).await;
    let b = board::build(&root, &events, &dag_id).unwrap();
    assert_eq!(b.campaign.id, dag_id);
    assert_eq!(b.campaign.intent, "board plan");
    let waves: Vec<(u32, Vec<&str>)> = b
        .waves
        .iter()
        .map(|w| (w.depth, w.nodes.iter().map(|n| n.node_id.as_str()).collect()))
        .collect();
    // The root (umbrella) node is left out; c is two deep via b.
    assert_eq!(waves, vec![(0, vec!["bd_a", "bd_d"]), (1, vec!["bd_b"]), (2, vec!["bd_c"])]);
    let card_a = &b.waves[0].nodes[0];
    assert_eq!(card_a.proof.total, 2);
    assert_eq!(card_a.proof.proven, 0, "checkboxes never count");
    assert_eq!(b.waves[0].nodes[1].status, CardStatus::Ready);
    assert_eq!(b.waves[1].nodes[0].status, CardStatus::Blocked);
    assert!(b.summary.next.iter().any(|c| c.node_id == "bd_d"));
    assert_eq!(b.waves[1].nodes[0].blocked_by, vec!["bd_a"]);
    assert_eq!(b.summary.proven.k, 0);
    assert_eq!(b.summary.proven.n, 2 + 1 + 1 + 1);

    // A receipt alone is not proof; a receipt followed by an accomplished
    // verdict is.
    let src = root.join("out.txt");
    std::fs::write(&src, "ok").unwrap();
    proof::add(&gate, &dag_id, "bd_a", "Export test passes", &src).await.unwrap();
    let mut events = all_events(&ledger).await;
    let b = board::build(&root, &events, &dag_id).unwrap();
    assert_eq!(b.waves[0].nodes[0].proof.proven, 0);
    events.push(verdict_event(&dag_id, "bd_a", "accomplished"));
    events.push(verdict_event(&dag_id, "bd_d", "accomplished"));
    let b = board::build(&root, &events, &dag_id).unwrap();
    assert_eq!(b.waves[0].nodes[0].proof.proven, 1);
    assert_eq!(b.waves[0].nodes[1].proof.proven, 1, "no contract: the verdict proves the node");
    assert_eq!(b.summary.proven.k, 2);

    // A manual wait-gate makes the card need you.
    gate.add_node_wait_gate(
        &dag_id,
        "bd_d",
        allternit_factory_engine::wait_gates::WaitGateKind::Manual,
        Some("Eoj signs off".into()),
        Default::default(),
        "test",
    )
    .await
    .unwrap();
    let events = all_events(&ledger).await;
    let b = board::build(&root, &events, &dag_id).unwrap();
    let d = b.summary.needs_you.iter().find(|c| c.node_id == "bd_d").expect("bd_d needs you");
    assert_eq!(d.status, CardStatus::NeedsYou);
    assert_eq!(d.gate.as_ref().unwrap().kind, "manual");

    assert!(board::build(&root, &events, "nope").is_err());
}

#[tokio::test]
async fn node_page_json_shape_is_camel_case() {
    let tmp = test_root();
    let root = tmp.path().to_path_buf();
    let (ledger, _r, gate) = build_gate(&root).await;
    let (_, dag_id, root_node) = gate.plan_new("page plan", None).await.unwrap();
    gate.plan_refine(&dag_id, "one", "agent", vec![node("np_a", &root_node, Some(STRUCTURED))])
        .await
        .unwrap();
    let src = root.join("shot.png");
    std::fs::write(&src, [1u8, 2, 3]).unwrap();
    let added = proof::add(&gate, &dag_id, "np_a", "Screenshot of the button", &src).await.unwrap();
    let mut events = all_events(&ledger).await;
    events.push(verdict_event(&dag_id, "np_a", "accomplished"));

    let page = node_page::build(&root, &events, &dag_id, "np_a").unwrap();
    let v = serde_json::to_value(&page).unwrap();
    for key in ["card", "spec", "progressMd", "proofMd", "files", "deliveries", "wih", "approval"] {
        assert!(v.get(key).is_some(), "missing {key}: {v}");
    }
    for key in ["dagId", "nodeId", "title", "status", "assignee", "bindingType", "proof", "blockedBy", "needsYou", "depth", "gate"] {
        assert!(v["card"].get(key).is_some(), "card missing {key}");
    }
    assert_eq!(v["card"]["status"], json!("ready"));
    assert_eq!(v["card"]["proof"], json!({ "proven": 1, "total": 2 }));
    assert_eq!(v["spec"]["intent"], json!("Ship the export button."));
    assert_eq!(v["spec"]["miniRequirements"], json!(["CSV only", "Max 10k rows"]));
    let contract = v["spec"]["proofContract"].as_array().unwrap();
    assert_eq!(contract[0]["checkedBy"], json!("cargo test export"));
    assert_eq!(contract[0]["evidence"], json!([]));
    assert_eq!(
        contract[1]["evidence"],
        json!([{ "path": added.path, "receiptId": added.receipt_id, "verdict": "accomplished" }])
    );
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0]["path"].as_str().unwrap().starts_with("proof/"));
    assert_eq!(files[0]["size"], json!(3));
    assert_eq!(v["deliveries"], json!([]));
    assert_eq!(v["approval"], json!(null));
    assert!(v["progressMd"].as_str().unwrap().starts_with("# Progress"));
    assert!(node_page::build(&root, &events, &dag_id, "ghost").is_err());
}
