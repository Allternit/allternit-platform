//! E5 flywheel + E6 vendor gating tests.

use super::backends::{self, DecisionBackend, Query};
use super::flywheel::{self, Bar, Metrics, Shadow, Truth, LIVE, SHADOW};
use super::{store, vendors};
use crate::db::DbHandle;
use serde_json::json;
use std::time::{Duration, Instant};

const OWNER: &str = "u_fly";

fn db() -> DbHandle {
    DbHandle::new_memory().unwrap()
}

/// Store one decision the way `finish` does, with its key and shadow pass.
fn decide(db: &DbHandle, id: &str, kind: &str, ctx: &str, served: Option<&str>) -> String {
    let ids = ["a", "b", "c"];
    let key = flywheel::key_hash(kind, ctx, None, &ids, &["A", "B", "C"]);
    let opts = json!([{ "id": "a", "text": "A" }, { "id": "b", "text": "B" }, { "id": "c", "text": "C" }]);
    let row = store::Row {
        id,
        owner: OWNER,
        created_at: &super::super::store::now(),
        kind,
        session_id: None,
        task: None,
        context_hash: ctx,
        has_image: false,
        options: &opts,
        choice: served,
        probs: &json!({}),
        confidence: 0.9,
        abstained: served.is_none(),
        backend: "local",
        latency_ms: 100.0,
        attempts: &json!([]),
    };
    store::insert(db, &row).unwrap();
    let s = Shadow { decision_id: id.into(), owner: OWNER.into(), kind: kind.into(), key: key.clone(), ids: ids.iter().map(|s| s.to_string()).collect(), has_image: false };
    flywheel::record_key(db, &s).unwrap();
    flywheel::record_shadow(db, &s).unwrap();
    key
}

fn outcome(db: &DbHandle, id: &str, status: &str, label: Option<&str>) {
    store::set_outcome(db, id, OWNER, status, label, None, &super::super::store::now()).unwrap().unwrap();
    flywheel::on_outcome(db, id, OWNER).unwrap();
}

fn shadow_choice(db: &DbHandle, id: &str) -> Option<String> {
    db.connect()
        .unwrap()
        .query_row("SELECT choice FROM decision_shadow WHERE decision_id = ?1 AND head = 'head.lookup'", [id], |r| r.get(0))
        .unwrap()
}

#[test]
fn decisions_shadow_records_next_to_served_and_learns_from_outcomes() {
    let db = db();
    decide(&db, "d1", "element", "ctx1", Some("b"));
    // First sight: the head has no answer, recorded as such, and it is shadow.
    assert_eq!(shadow_choice(&db, "d1"), None);
    let conn = db.connect().unwrap();
    assert_eq!(flywheel::current_state(&conn, "head.lookup", "element").as_deref(), Some(SHADOW));
    outcome(&db, "d1", "success", Some("b"));
    // Same decision again: the head answers with what succeeded.
    decide(&db, "d2", "element", "ctx1", Some("b"));
    assert_eq!(shadow_choice(&db, "d2").as_deref(), Some("b"));
    // Another context: no answer.
    decide(&db, "d3", "element", "ctx2", Some("a"));
    assert_eq!(shadow_choice(&db, "d3"), None);
    // A re-patched outcome is not learned twice.
    outcome(&db, "d1", "success", Some("b"));
    let s: i64 = conn.query_row("SELECT successes FROM decision_lookup WHERE choice = 'b'", [], |r| r.get(0)).unwrap();
    assert_eq!(s, 1);
    // Failures outweigh: the head stops answering that choice.
    for (i, id) in ["f1", "f2"].iter().enumerate() {
        decide(&db, id, "element", "ctx1", Some("b"));
        if i == 0 {
            assert_eq!(shadow_choice(&db, id).as_deref(), Some("b"));
        }
        outcome(&db, id, "failure", Some("b"));
    }
    decide(&db, "d4", "element", "ctx1", Some("c"));
    assert_eq!(shadow_choice(&db, "d4"), None);
    let m = flywheel::metrics(&conn, "head.lookup", "element", "", 1000).unwrap();
    // d2 has no outcome; f1 answered "b", which failed.
    assert_eq!((m.decisions, m.answered, m.labelled, m.correct), (6, 2, 1, 0));
}

#[test]
fn decisions_outcome_learned_before_the_shadow_pass_is_not_counted() {
    let db = db();
    decide(&db, "d1", "element", "ctx", Some("a"));
    outcome(&db, "d1", "success", None);
    // A late shadow pass for d1 would see its own outcome: skipped.
    db.connect().unwrap().execute("DELETE FROM decision_shadow", []).unwrap();
    let s = Shadow { decision_id: "d1".into(), owner: OWNER.into(), kind: "element".into(), key: flywheel::key_hash("element", "ctx", None, &["a", "b", "c"], &["A", "B", "C"]), ids: vec!["a".into(), "b".into(), "c".into()], has_image: false };
    flywheel::record_shadow(&db, &s).unwrap();
    let n: i64 = db.connect().unwrap().query_row("SELECT COUNT(*) FROM decision_shadow", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn decisions_truth_from_outcomes() {
    assert_eq!(flywheel::truth(Some("a"), Some("success"), None), Truth::Right("a".into()));
    assert_eq!(flywheel::truth(Some("a"), Some("success"), Some("b")), Truth::Right("b".into()));
    assert_eq!(flywheel::truth(Some("a"), Some("failure"), Some("a")), Truth::Wrong("a".into()));
    assert_eq!(flywheel::truth(Some("a"), Some("failure"), Some("b")), Truth::Right("b".into()));
    assert_eq!(flywheel::truth(Some("a"), Some("error"), Some("a")), Truth::Unknown);
    assert_eq!(flywheel::truth(None, Some("failure"), None), Truth::Unknown);
}

fn m(labelled: u64, correct: u64, overconf: f64) -> Metrics {
    Metrics {
        labelled,
        correct,
        accuracy: (labelled > 0).then(|| correct as f64 / labelled as f64),
        overconfidence: Some(overconf),
        ..Default::default()
    }
}

#[test]
fn decisions_promote_and_demote_thresholds_with_hysteresis() {
    let b = Bar::default();
    assert_eq!((b.min_samples, b.promote_at, b.demote_at), (200, 0.97, 0.93));
    // Under the sample minimum: stays shadow, however accurate.
    assert_eq!(flywheel::next_state(SHADOW, &m(199, 199, 0.0), &b), None);
    assert_eq!(flywheel::next_state(SHADOW, &m(200, 194, -0.2), &b).map(|x| x.0), Some(LIVE));
    assert_eq!(flywheel::next_state(SHADOW, &m(200, 193, -0.2), &b), None);
    // Overconfident heads do not go live.
    assert_eq!(flywheel::next_state(SHADOW, &m(200, 200, 0.06), &b), None);
    // Hysteresis: a live head between demote_at and promote_at stays live.
    assert_eq!(flywheel::next_state(LIVE, &m(100, 95, 0.0), &b), None);
    assert_eq!(flywheel::next_state(LIVE, &m(100, 92, 0.0), &b).map(|x| x.0), Some(SHADOW));
    assert_eq!(flywheel::next_state(LIVE, &m(49, 0, 0.0), &b), None);
    // Retired heads never move by themselves; safety never auto-promotes.
    assert_eq!(flywheel::next_state("retired", &m(1000, 1000, 0.0), &b), None);
    assert!(!flywheel::bar("safety").auto_promote);
    assert_eq!(flywheel::next_state(SHADOW, &m(1000, 1000, 0.0), &flywheel::bar("safety")), None);
}

#[test]
fn decisions_head_earns_promotion_then_drops_on_drift_audited() {
    let db = db();
    let b = Bar { min_samples: 3, min_demote_samples: 3, ..Bar::default() };
    let conn = db.connect().unwrap();
    // Teach one context, then the head answers it right three times in shadow.
    decide(&db, "t0", "element", "ctx", Some("a"));
    outcome(&db, "t0", "success", None);
    for i in 0..3 {
        let id = format!("s{i}");
        decide(&db, &id, "element", "ctx", Some("a"));
        store::set_outcome(&db, &id, OWNER, "success", Some("a"), None, &super::super::store::now()).unwrap();
    }
    // Overconfidence is computed against the lookup's smoothed probs (< 1): fine.
    assert_eq!(flywheel::evaluate_with(&conn, "element", &b).unwrap(), vec![("head.lookup".to_string(), LIVE.to_string())]);
    // Right after the transition the window is empty: no immediate flip back.
    assert!(flywheel::evaluate_with(&conn, "element", &b).unwrap().is_empty());
    std::thread::sleep(Duration::from_millis(5));
    // Drift: the head keeps answering "a", the world now says "b".
    for i in 0..3 {
        let id = format!("w{i}");
        decide(&db, &id, "element", "ctx", Some("a"));
        store::set_outcome(&db, &id, OWNER, "failure", Some("b"), None, &super::super::store::now()).unwrap();
    }
    assert_eq!(flywheel::evaluate_with(&conn, "element", &b).unwrap(), vec![("head.lookup".to_string(), SHADOW.to_string())]);
    let audit: Vec<(String, String, String)> = conn
        .prepare("SELECT from_state, to_state, actor FROM decision_head_audit ORDER BY at")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(audit, vec![("shadow".into(), "live".into(), "flywheel".into()), ("live".into(), "shadow".into(), "flywheel".into())]);
    let listed = flywheel::list(&conn).unwrap();
    assert_eq!(listed["data"][0]["state"], "shadow");
    assert_eq!(listed["data"][0]["audit"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn decisions_live_head_leads_the_chain_and_answers_under_10ms() {
    let db = db();
    decide(&db, "t0", "element", "ctx", Some("b"));
    outcome(&db, "t0", "success", None);
    let key = flywheel::key_hash("element", "ctx", None, &["a", "b", "c"], &["A", "B", "C"]);
    let chain = || vec![backends::backend("head").unwrap(), backends::backend("local").unwrap()];
    // Shadow: the head slot is dropped from the served chain.
    let names: Vec<_> = flywheel::expand(chain(), &db, OWNER, "element", &key).iter().map(|b| b.name()).collect();
    assert_eq!(names, vec!["local"]);
    let conn = db.connect().unwrap();
    flywheel::transition(&conn, "head.lookup", "element", LIVE, "test", "test", &Metrics::default()).unwrap();
    let served = flywheel::expand(chain(), &db, OWNER, "element", &key);
    assert_eq!(served.iter().map(|b| b.name()).collect::<Vec<_>>(), vec!["head.lookup", "local"]);
    // Fill the lookup table so the read is not trivially small.
    conn.execute_batch("BEGIN").unwrap();
    for i in 0..5000 {
        conn.execute(
            "INSERT INTO decision_lookup (owner, kind, key_hash, choice, successes, failures, updated_at) VALUES (?1, 'element', ?2, 'a', 1, 0, 'x')",
            rusqlite::params![format!("o{}", i % 50), format!("k{i}")],
        )
        .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    let q = Query { kind: "element", context: "ctx", question: None, image: None, ids: vec!["a", "b", "c"], texts: vec!["A", "B", "C"], allow_abstain: true };
    let mut lat = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        let a = served[0].decide(&q).await.unwrap();
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(a.probs[1] > 0.6 && a.probs[0] < 0.2);
    }
    lat.sort_by(f64::total_cmp);
    let p95 = lat[189];
    eprintln!("lookup head p50 {:.3} ms, p95 {:.3} ms", lat[100], p95);
    assert!(p95 < 10.0, "lookup head p95 {p95} ms");
    // A miss escalates (an error attempt, not an abstention).
    let miss = Query { context: "other", ..q };
    let other = flywheel::expand(chain(), &db, OWNER, "element", "nokey");
    assert_eq!(other[0].decide(&miss).await.err().as_deref(), Some("no match"));
}

#[test]
fn decisions_vendors_off_by_default_and_enabled_per_project() {
    // No config: off for every project, even when the key would be set.
    for b in ["vendor", "typesafe"] {
        assert!(!vendors::allowed_in(None, b, Some("p1")));
        assert!(!vendors::allowed_in(None, b, None));
    }
    let cfg = r#"{"vendor": ["p1"], "typesafe": ["*"]}"#;
    assert!(vendors::allowed_in(Some(cfg), "vendor", Some("p1")));
    assert!(!vendors::allowed_in(Some(cfg), "vendor", Some("p2")));
    assert!(!vendors::allowed_in(Some(cfg), "vendor", None));
    assert!(vendors::allowed_in(Some(cfg), "typesafe", None));
    // Our own backends are never gated.
    assert!(vendors::allowed_in(None, "local", None) && vendors::allowed_in(None, "oracle", None));
    // Without a key the adapters are not even enabled.
    if std::env::var("ALLTERNIT_DECISIONS_VENDOR_KEY").is_err() {
        assert!(!backends::backend("vendor").unwrap().enabled());
    }
    if std::env::var("ALLTERNIT_DECISIONS_TYPESAFE_KEY").is_err() {
        assert!(!backends::backend("typesafe").unwrap().enabled());
    }
}

#[test]
fn decisions_vendor_wire_formats() {
    let q = Query { kind: "element", context: "ctx", question: Some("Which?"), image: Some("data:image/png;base64,AA=="), ids: vec!["o1", "o2"], texts: vec!["One", "Two"], allow_abstain: false };
    let r = vendors::OpenAiDecisions::request(&q, "m");
    assert_eq!(r["questions"][0]["choices"][1], json!({ "value": "o2", "description": "Two" }));
    assert_eq!(r["input"][0]["content"][1]["type"], "input_image");
    let a = vendors::OpenAiDecisions::parse(
        &json!({ "answers": [{ "type": "choice", "name": "decision", "choice": "o2",
            "probabilities": [{ "value": "o1", "probability": 0.1 }, { "value": "o2", "probability": 0.9 }], "confidence": 0.85 }] }),
        &q.ids,
    )
    .unwrap();
    assert_eq!(a.probs, vec![0.1, 0.9]);
    assert!(vendors::OpenAiDecisions::parse(&json!({ "answers": [{ "type": "refusal", "name": "decision" }] }), &q.ids).unwrap().abstain);
    let r = vendors::TypeSafeJev::request(&q, "jev-latest");
    assert_eq!(r["questions"]["decision"]["criteria"]["o1"], "One");
    assert_eq!(r["state"], "ctx");
    let a = vendors::TypeSafeJev::parse(&json!({ "answers": { "decision": { "type": "choice", "choice": "o1", "probabilities": { "o1": 0.7, "o2": 0.3 } } } }), &q.ids).unwrap();
    assert_eq!(a.probs, vec![0.7, 0.3]);
    assert!(vendors::TypeSafeJev::parse(&json!({ "answers": { "decision": { "probabilities": { "zz": 1.0 } } } }), &q.ids).is_err());
}
