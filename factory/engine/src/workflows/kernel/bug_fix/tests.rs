use super::*;
use serde_json::json;

#[test]
fn wp10_template_passes_all_seven_invariants_and_covers_packs() {
    let reg = PrimitiveRegistry::global();
    let g = instantiate("task.fixture", &["fs:src/math.ts".into()]).unwrap();
    assert!(graph::validate(&g, reg).is_empty());
    for i in 0..24 { assert!(g.node(&format!("N{i:02}")).is_some()); }
    for name in ["deep_hypothesis", "deep_patch", "deep_repair", "deep_review"] {
        assert_eq!(g.node(name).unwrap().cognitive_role.as_deref(), Some("S3"));
    }
    let packs = reg.active_packs(&["typescript"], true, true, true);
    assert_eq!(packs.len(), 7);
    for n in &g.nodes {
        assert!(packs.iter().any(|p| p.primitives.contains(&n.primitive_id)), "unpacked {}", n.primitive_id);
    }
    assert_eq!(reg.active_packs(&[], false, false, false).len(), 3);
    assert!(instantiate("", &["fs:a".into()]).is_err());
    assert!(instantiate("task", &[]).is_err());
    assert!(instantiate("task", &["projection:a".into()]).is_err());
    let p = crate::kernel::projection::project(&g);
    assert_eq!(p.tasks.iter().find(|n| n.node_id == "N15").unwrap().failure_targets, ["N16"]);
    assert_eq!(p.tasks.iter().find(|n| n.node_id == "N17").unwrap().loop_targets, ["N12"]);
}

#[test]
fn wp10_error_bank_and_fail_closed_ladder() {
    let bank = error_ontology();
    assert_eq!(bank.classes.len(), 18);
    assert_eq!(bank.primitive_id, "dec.classify_error");
    assert_eq!(error_class("TEST_ASSERTION"), "TEST_ASSERTION");
    assert_eq!(error_class("ERR_INTERNAL"), "UNKNOWN");
    let ladder = verification_ladder();
    assert_eq!(ladder.iter().map(|s| s.step_id.as_str()).collect::<Vec<_>>(),
        ["parse", "structure", "format", "lint", "typecheck", "target_test", "affected_tests", "integration_build", "semantic_requirement", "diff_review", "regression"]);
    for s in &ladder { assert!(PrimitiveRegistry::global().contains(&s.primitive_id)); }
    let evidence = json!({"deterministic":true,"evidence_refs":["receipt:test"]});
    assert!(step_passes("PASS", true, &evidence));
    for r in ["FAIL", "ERROR", "INCONCLUSIVE", "NOT_APPLICABLE"] { assert!(!step_passes(r, true, &evidence)); }
    assert!(!step_passes("PASS", true, &json!({"deterministic":false,"evidence_refs":["r"]})));
    assert!(!step_passes("PASS", false, &json!({"evidence_refs":[]})));
}

#[test]
fn wp10_agency_entry_point_matches_wp11_template_graph_shape() {
    let t = agency_graph("fix add()", &json!({"workspace": "/tmp/ws"})).unwrap();
    assert_eq!(t.nodes.len(), graph().unwrap().nodes.len());
    let n14 = t.nodes.iter().find(|n| n["id"] == "N14").unwrap();
    assert_eq!((n14["role"].as_str(), n14["writes"].as_bool()), (Some("mut.apply_patch_transactionally"), Some(true)));
    assert_eq!(n14["write_set"], json!(["fs:/tmp/ws"]));
    assert!(t.edges.iter().all(|e| e["from"].is_string() && e["to"].is_string()));
    assert_eq!(t.wih_policy["requires_lease_for_write"], true);
    assert_eq!(t.wih_policy["task_id"], agency_graph("fix add()", &json!({"workspace": "/x"})).unwrap().wih_policy["task_id"]);
    let explicit = agency_graph("g", &json!({"writable_resources": ["fs:src/a.ts"], "task_id": "task.x"})).unwrap();
    assert_eq!(explicit.wih_policy["write_set"], json!(["fs:src/a.ts"]));
    assert!(agency_graph("g", &json!({})).is_err(), "no declared write authority fails closed");
    assert_eq!((TEMPLATE_ID, TEMPLATE_VERSION, TEMPLATE_SOURCE, COMPLETION_POLICY), ("BUG_FIX", 1, "kernel", "completion.bug_fix"));
}

#[tokio::test]
async fn s0_reconcile_returns_class_and_tolerates_missing_result() {
    let off = crate::kernel::s1_outcome::OutcomeReporter { enabled: false, ..crate::kernel::s1_outcome::OutcomeReporter::from_env() };
    assert_eq!(reconcile_s0_classification(&off, None, "TEST_ASSERTION"), "TEST_ASSERTION");
    assert_eq!(reconcile_s0_classification(&off, None, "ERR_INTERNAL"), "UNKNOWN");
}
