//! B5: the replay acceptance rule for promotions.

use super::acceptance::{self, Evidence};
use super::flywheel::{self, Bar, LIVE, SHADOW};
use super::flywheel_tests::{db, decide, outcome, OWNER};
use super::store;
use serde_json::{json, Value};

fn ev(id: &str, at: &str, acc: f64, base: f64, cost: (f64, f64), lat: (f64, f64)) -> Evidence {
    Evidence {
        id: id.into(),
        head: "head.lookup".into(),
        kind: "element".into(),
        head_version: "1".into(),
        owner: "admin".into(),
        created_at: at.into(),
        runs: 4,
        decisions: 20,
        labelled: 10,
        accuracy: Some(acc),
        baseline_accuracy: Some(base),
        cost_usd: Some(cost.0),
        baseline_cost_usd: Some(cost.1),
        latency_ms: Some(lat.0),
        baseline_latency_ms: Some(lat.1),
        pass_at_1: Some(acc),
        pass_at_k: Some(acc),
        k: 5,
        deterministic: true,
        metrics: json!({ "accuracy": acc }),
    }
}

fn check<'a>(a: &'a acceptance::Acceptance, name: &str) -> &'a acceptance::Check {
    a.checks.iter().find(|c| c.name == name).unwrap()
}

#[test]
fn decisions_acceptance_passes_on_accuracy_within_epsilon_and_a_cheaper_or_faster_head() {
    let b = Bar::default();
    assert_eq!((b.replay_epsilon, b.replay_k), (0.01, 5));
    // Same accuracy, cheaper: pass.
    let a = acceptance::judge(&ev("rpb_1", "t", 0.9, 0.9, (0.0, 0.002), (5.0, 5.0)), &b);
    assert!(a.passed, "{}", a.summary());
    // Exactly baseline − ε, only faster: pass (edge is inclusive).
    let a = acceptance::judge(&ev("rpb_2", "t", 0.89, 0.90, (0.0, 0.0), (1.0, 40.0)), &b);
    assert!(a.passed, "{}", a.summary());
    // More accurate but neither cheaper nor faster: fail on cost_or_latency only.
    let a = acceptance::judge(&ev("rpb_3", "t", 1.0, 0.9, (0.01, 0.01), (40.0, 40.0)), &b);
    assert!(!a.passed && check(&a, "accuracy").passed && !check(&a, "cost_or_latency").passed);
    assert!(a.summary().contains("not better"), "{}", a.summary());
}

#[test]
fn decisions_acceptance_fails_below_epsilon_without_labels_or_without_a_baseline() {
    let b = Bar::default();
    let a = acceptance::judge(&ev("rpb_4", "t", 0.88, 0.90, (0.0, 0.01), (1.0, 40.0)), &b);
    assert!(!a.passed && !check(&a, "accuracy").passed && check(&a, "cost_or_latency").passed);
    // A wider ε (configurable per kind) admits it.
    let wide = Bar { replay_epsilon: 0.05, ..Bar::default() };
    assert!(acceptance::judge(&ev("rpb_4", "t", 0.88, 0.90, (0.0, 0.01), (1.0, 40.0)), &wide).passed);
    let mut none = ev("rpb_5", "t", 0.9, 0.9, (0.0, 0.01), (1.0, 40.0));
    none.labelled = 0;
    none.accuracy = None;
    let a = acceptance::judge(&none, &b);
    assert!(!a.passed && !check(&a, "labelled").passed && !check(&a, "accuracy").passed);
    let mut no_base = ev("rpb_6", "t", 0.9, 0.9, (0.0, 0.01), (1.0, 40.0));
    no_base.baseline_accuracy = None;
    assert!(!acceptance::judge(&no_base, &b).passed);
}

#[test]
fn decisions_pass_at_k_deterministic_equals_pass_at_1() {
    // One sample per decision (deterministic heads): pass@k is pass@1.
    assert_eq!(acceptance::pass_at(&[(1, 1), (1, 0), (1, 1), (1, 1)], 5), (Some(0.75), Some(0.75)));
    assert_eq!(acceptance::pass_at(&[], 5), (None, None));
    // Nondeterministic: 5 samples, 1 right → pass@1 0.2, pass@5 1.0; none right → 0.
    let (p1, pk) = acceptance::pass_at(&[(5, 1), (5, 0)], 5);
    assert!((p1.unwrap() - 0.1).abs() < 1e-9 && (pk.unwrap() - 0.5).abs() < 1e-9, "{p1:?} {pk:?}");
    // k = 2 of 5 with 1 right: 1 - C(4,2)/C(5,2) = 0.4.
    let (_, pk) = acceptance::pass_at(&[(5, 1)], 2);
    assert!((pk.unwrap() - 0.4).abs() < 1e-9, "{pk:?}");
}

fn earn_promotion(db: &crate::db::DbHandle) {
    decide(db, "t0", "element", "ctx", Some("a"));
    outcome(db, "t0", "success", None);
    for i in 0..3 {
        let id = format!("s{i}");
        decide(db, &id, "element", "ctx", Some("a"));
        store::set_outcome(db, &id, OWNER, "success", Some("a"), None, &super::super::store::now()).unwrap();
    }
}

#[test]
fn decisions_promotion_needs_passing_replay_evidence_and_audits_its_id() {
    let db = db();
    let b = Bar { min_samples: 3, min_demote_samples: 3, ..Bar::default() };
    let conn = db.connect().unwrap();
    earn_promotion(&db);
    // The latest replay set says the head is worse than what is served: held in shadow.
    acceptance::record(&conn, &ev("rpb_old", "2026-10-01T00:00:00.000Z", 0.99, 0.99, (0.0, 0.01), (1.0, 50.0))).unwrap();
    acceptance::record(&conn, &ev("rpb_bad", "2026-10-02T00:00:00.000Z", 0.70, 0.95, (0.0, 0.01), (1.0, 50.0))).unwrap();
    assert!(flywheel::evaluate_with(&conn, "element", &b).unwrap().is_empty(), "held back by the latest evidence");
    assert_eq!(flywheel::current_state(&conn, "head.lookup", "element").as_deref(), Some(SHADOW));
    let x = acceptance::explain_with(&conn, "head.lookup", "element", &b).unwrap();
    assert_eq!(x["passed"], false);
    assert_eq!(x["replay"]["evidence"]["id"], "rpb_bad");
    assert!(x["reason"].as_str().unwrap().contains("replay evidence rpb_bad failed"), "{x}");
    // A newer, passing replay set: promoted, and the audit row names it.
    acceptance::record(&conn, &ev("rpb_good", "2026-10-03T00:00:00.000Z", 0.96, 0.95, (0.0, 0.01), (1.0, 50.0))).unwrap();
    assert!(acceptance::explain_with(&conn, "head.lookup", "element", &b).unwrap()["passed"] == true);
    assert_eq!(flywheel::evaluate_with(&conn, "element", &b).unwrap(), vec![("head.lookup".to_string(), LIVE.to_string())]);
    let (reason, m): (String, String) = conn
        .query_row("SELECT reason, metrics_json FROM decision_head_audit WHERE to_state = 'live'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let m: Value = serde_json::from_str(&m).unwrap();
    assert_eq!(m["replay_evidence_id"], "rpb_good");
    assert_eq!(m["acceptance"]["passed"], true);
    assert_eq!(m["labelled"], 3, "the live metrics are still in the audit row");
    assert!(reason.contains("replay rpb_good"), "{reason}");
}

#[test]
fn decisions_promotion_without_a_replay_set_keeps_the_live_bar_alone() {
    let db = db();
    let b = Bar { min_samples: 3, min_demote_samples: 3, ..Bar::default() };
    let conn = db.connect().unwrap();
    earn_promotion(&db);
    let x = acceptance::explain_with(&conn, "head.lookup", "element", &b).unwrap();
    assert_eq!(x["replay"], Value::Null);
    // Evidence for another kind does not count for this one.
    let mut other = ev("rpb_x", "2026-10-02T00:00:00.000Z", 0.1, 0.9, (1.0, 0.0), (90.0, 1.0));
    other.kind = "verify".into();
    acceptance::record(&conn, &other).unwrap();
    assert_eq!(flywheel::evaluate_with(&conn, "element", &b).unwrap(), vec![("head.lookup".to_string(), LIVE.to_string())]);
    let m: String = conn.query_row("SELECT metrics_json FROM decision_head_audit", [], |r| r.get(0)).unwrap();
    assert!(serde_json::from_str::<Value>(&m).unwrap().get("replay_evidence_id").is_none());
}
