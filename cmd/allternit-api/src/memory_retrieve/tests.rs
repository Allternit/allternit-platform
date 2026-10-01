use super::eval::{run_eval, Fixture};
use super::*;
use crate::memory_index::EmbedClient;
use crate::memory_relations::{write_relation, NodeKind, RelationType, ORIGIN_INCUMBENT};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Brain = Arc<dyn Fn(&str, &str) -> (String, f64) + Send + Sync>;

fn db() -> DbHandle {
    DbHandle::new_memory().expect("memory db")
}

/// Mock decision runtime: `/v1/decision` answers `brain(bank, state)` with
/// ids dec-1, dec-2, …; `/v1/decision/outcome` answers {}. Every request is
/// sent as "path|body".
async fn mock_s1(brain: Brain) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let (tx, n, brain) = (tx.clone(), n.clone(), brain.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let mut got = Vec::new();
                let (path, body) = loop {
                    let r = s.read(&mut buf).await.unwrap_or(0);
                    if r == 0 {
                        return;
                    }
                    got.extend_from_slice(&buf[..r]);
                    let txt = String::from_utf8_lossy(&got).to_string();
                    if let Some(i) = txt.find("\r\n\r\n") {
                        let len = txt
                            .lines()
                            .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if got.len() >= i + 4 + len {
                            let path = txt.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
                            break (path, txt[i + 4..].to_string());
                        }
                    }
                };
                let resp = if path == "/v1/decision" {
                    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let (ans, conf) = brain(v["request"]["decision_bank_id"].as_str().unwrap_or(""), v["state"].as_str().unwrap_or(""));
                    let id = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    json!({ "answer": ans, "confidence": conf, "extensions": { "x-decision_id": format!("dec-{id}") } }).to_string()
                } else {
                    "{}".to_string()
                };
                let _ = tx.send(format!("{path}|{body}"));
                let _ = s
                    .write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{resp}", resp.len()).as_bytes())
                    .await;
            });
        }
    });
    (url, rx)
}

fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Vec<(String, Value)> {
    let mut out = vec![];
    while let Ok(m) = rx.try_recv() {
        let (p, b) = m.split_once('|').unwrap();
        out.push((p.to_string(), serde_json::from_str(b).unwrap_or(Value::Null)));
    }
    out
}

/// The memory text inside an IS_RELEVANT / RANK state.
fn memory_of(state: &str) -> &str {
    state.split_once("):\n").map(|(_, m)| m).unwrap_or(state)
}

fn fact(db: &DbHandle, id: &str, text: &str) {
    let conn = db.connect().unwrap();
    conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, 'u1', ?2, 0.9)", params![id, text]).unwrap();
    kernel::store_embedding(db, "u1", "fact", id, text).unwrap();
}

fn obs(db: &DbHandle, text: &str) -> String {
    kernel::record_observation(db, "u1", None, None, "turn_user", text, Some("user")).unwrap()
}

fn seed(db: &DbHandle) {
    fact(db, "f_austin", "Maya lives in Austin.");
    fact(db, "f_denver", "Maya lives in Denver now.");
    fact(db, "f_car", "Maya drives a blue Subaru.");
    fact(db, "f_tires", "The Subaru needs new tires.");
    let conn = db.connect().unwrap();
    write_relation(&conn, "u1", (NodeKind::Fact, "f_denver"), RelationType::Updates, (NodeKind::Fact, "f_austin"), 0.9, ORIGIN_INCUMBENT, None).unwrap();
    write_relation(&conn, "u1", (NodeKind::Fact, "f_tires"), RelationType::AboutEntity, (NodeKind::Fact, "f_car"), 0.9, ORIGIN_INCUMBENT, None).unwrap();
}

fn always(bank_answers: &'static [(&'static str, &'static str)]) -> Brain {
    Arc::new(move |bank, _| (bank_answers.iter().find(|(b, _)| *b == bank).map(|(_, a)| a.to_string()).unwrap_or_default(), 0.9))
}

#[test]
fn route_answers_parse_to_spaces_with_facts_always_included() {
    assert_eq!(parse_route("all"), Space::ALL.to_vec());
    assert_eq!(parse_route("notes"), vec![Space::Facts, Space::Notes]);
    assert_eq!(parse_route("[\"documents\", \"procedures\"]"), vec![Space::Facts, Space::Documents, Space::Procedures]);
    assert_eq!(parse_route("facts"), vec![Space::Facts]);
    assert_eq!(Space::parse("note"), Some(Space::Notes));
}

#[tokio::test]
async fn s1_route_picks_the_spaces_searched() {
    let db = db();
    seed(&db);
    db.connect()
        .unwrap()
        .execute("INSERT INTO memory_notes (id, user_id, note_type, title, content) VALUES ('n1', 'u1', 'general', 'Subaru', 'Tire shop on 5th sells Subaru tires')", [])
        .unwrap();
    let (url, mut rx) = mock_s1(always(&[(BANK_ROUTE, "notes"), (BANK_IS_RELEVANT, "relevant"), (BANK_RANK, "high")])).await;
    let out = retrieve(&S1Client::new(&url), &db, "u1", None, "rq1", "Subaru tires", None, &RetrieveConfig::with_k(5)).await.unwrap();
    assert_eq!(out.spaces, vec![Space::Facts, Space::Notes]);
    assert_eq!(out.route_decision.as_deref(), Some("dec-1"));
    assert!(out.evidence.iter().any(|e| e.item.id == "n1" && e.step == "anchor:notes"), "{:?}", out.evidence);
    let reqs = drain(&mut rx);
    assert_eq!(reqs[0].1["request"]["decision_bank_id"], BANK_ROUTE);
    assert_eq!(reqs[0].1["request"]["extensions"]["x-motif"], "ROUTE");
    assert!(reqs.iter().any(|(_, b)| b["request"]["extensions"]["x-motif"] == "GATE"));
}

#[tokio::test]
async fn evidence_loop_stops_when_enough_and_is_bounded_otherwise() {
    let db = db();
    for i in 0..8 {
        obs(&db, &format!("Maya practiced cello scales on day {i}"));
    }
    let mut cfg = RetrieveConfig::with_k(5);
    cfg.max_checks = 4;
    cfg.enough_evidence = 2;
    // Never relevant: CONTINUE until the loop bound.
    let (url, _rx) = mock_s1(always(&[(BANK_ROUTE, "observations"), (BANK_IS_RELEVANT, "not_relevant"), (BANK_RANK, "high")])).await;
    let out = retrieve(&S1Client::new(&url), &db, "u1", None, "rq", "cello scales", None, &cfg).await.unwrap();
    assert_eq!(out.checks, 4);
    assert!(!out.stopped_early);
    assert!(out.evidence.is_empty());
    // Always relevant: STOP after `enough_evidence`.
    let (url, _rx) = mock_s1(always(&[(BANK_ROUTE, "observations"), (BANK_IS_RELEVANT, "relevant"), (BANK_RANK, "high")])).await;
    let out = retrieve(&S1Client::new(&url), &db, "u1", None, "rq", "cello scales", None, &cfg).await.unwrap();
    assert_eq!(out.checks, 2);
    assert!(out.stopped_early);
    assert_eq!(out.evidence.len(), 2);
    // Low confidence does not count as evidence.
    let low: Brain = Arc::new(|b, _| (match b { BANK_ROUTE => "observations", BANK_IS_RELEVANT => "relevant", _ => "high" }.into(), 0.3));
    let (url, _rx) = mock_s1(low).await;
    let out = retrieve(&S1Client::new(&url), &db, "u1", None, "rq", "cello scales", None, &cfg).await.unwrap();
    assert_eq!((out.checks, out.evidence.len()), (4, 0));
}

#[tokio::test]
async fn expansion_follows_typed_edges_and_skips_superseded_facts() {
    let db = db();
    seed(&db);
    let conn = db.connect().unwrap();
    let n: Vec<String> = neighbours(&conn, "u1", None, "f_denver", false).unwrap().into_iter().map(|(_, r)| r.id).collect();
    assert!(n.is_empty(), "superseded Austin fact must be skipped: {n:?}");
    let n: Vec<(String, String)> = neighbours(&conn, "u1", None, "f_denver", true).unwrap().into_iter().map(|(rel, r)| (rel, r.id)).collect();
    assert_eq!(n, vec![("updates".to_string(), "f_austin".to_string())]);
    drop(conn);
    // Anchor on the car; the tires fact arrives by expansion.
    let brain: Brain = Arc::new(|b, s| match b {
        BANK_ROUTE => ("facts".into(), 0.9),
        BANK_IS_RELEVANT => ((if memory_of(s).contains("drives") { "relevant" } else { "not_relevant" }).into(), 0.9),
        _ => ("high".into(), 0.9),
    });
    let (url, _rx) = mock_s1(brain).await;
    let out = retrieve(&S1Client::new(&url), &db, "u1", None, "rq", "which car does Maya drive", None, &RetrieveConfig::with_k(5)).await.unwrap();
    let steps: Vec<(String, String)> = out.evidence.iter().map(|e| (e.item.id.clone(), e.step.clone())).collect();
    assert!(steps.contains(&("f_car".into(), "anchor:facts".into())), "{steps:?}");
    assert!(steps.contains(&("f_tires".into(), "expansion:about_entity".into())), "{steps:?}");
    assert!(!steps.iter().any(|(id, _)| id == "f_austin"));
}

#[tokio::test]
async fn rank_selects_and_orders_the_final_evidence() {
    let db = db();
    seed(&db);
    let brain: Brain = Arc::new(|b, s| match b {
        BANK_ROUTE => ("facts".into(), 0.9),
        BANK_IS_RELEVANT => ("relevant".into(), 0.9),
        _ => {
            let m = memory_of(s);
            ((if m.contains("Denver") { "high" } else if m.contains("Subaru") { "medium" } else { "low" }).into(), 0.8)
        }
    });
    let (url, _rx) = mock_s1(brain).await;
    let mut cfg = RetrieveConfig::with_k(5);
    cfg.enough_evidence = 10;
    let out = retrieve(&S1Client::new(&url), &db, "u1", None, "rq", "Maya lives Denver Subaru", None, &cfg).await.unwrap();
    let ids: Vec<&str> = out.evidence.iter().map(|e| e.item.id.as_str()).collect();
    assert_eq!(ids.first(), Some(&"f_denver"), "{ids:?}");
    assert!(ids.iter().all(|id| *id != "f_austin"));
    assert!(out.evidence.iter().all(|e| e.rank_decision.is_some()));
    assert!(out.evidence[0].score > out.evidence.last().unwrap().score || out.evidence.len() == 1);
}

#[tokio::test]
async fn shadow_leaves_the_incumbent_unchanged_and_answer_labels_it() {
    let db = db();
    seed(&db);
    let q = "where does Maya live";
    let (incumbent, recall_id) = kernel::recall_logged(&db, "u1", None, Some("sess-1"), q, None, 5).unwrap();
    assert!(!incumbent.is_empty());
    let before: Vec<String> = incumbent.iter().map(|r| r.id.clone()).collect();
    let brain: Brain = Arc::new(|b, s| match b {
        BANK_ROUTE => ("all".into(), 0.9),
        BANK_IS_RELEVANT => ((if memory_of(s).contains("lives") { "relevant" } else { "not_relevant" }).into(), 0.9),
        _ => ("high".into(), 0.9),
    });
    let (url, mut rx) = mock_s1(brain).await;
    let client = S1Client::new(&url);
    let summary = shadow_recall(&client, &db, "u1", None, &recall_id, q, None, &incumbent, &RetrieveConfig::with_k(5)).await.unwrap();
    assert_eq!(summary.incumbent, before);
    assert!(summary.would_be.contains(&"f_denver".to_string()));
    // The incumbent is untouched: same input, same set.
    let (again, _) = kernel::recall_logged(&db, "u1", None, Some("sess-1"), q, None, 5).unwrap();
    assert_eq!(again.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), before);
    let conn = db.connect().unwrap();
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM memory_s1_decisions WHERE observation_id = ?1", params![recall_id], |r| r.get(0)).unwrap();
    assert_eq!(rows as usize, summary.decisions + summary.would_be.len());
    let selected: i64 = conn
        .query_row("SELECT COUNT(*) FROM memory_s1_decisions WHERE observation_id = ?1 AND bank = ?2 AND s1_answer LIKE 'anchor:%'", params![recall_id, BANK_SELECTED], |r| r.get(0))
        .unwrap();
    assert!(selected >= 1);
    drop(conn);
    drain(&mut rx);

    // The answer restates Denver: that evidence is `used`, the rest `unused`.
    let label = label_from_answer(&client, &db, "u1", "sess-1", "She lives in Denver now, Maya moved there.").await;
    assert_eq!(label.recall_id.as_deref(), Some(recall_id.as_str()));
    assert!(label.used.contains(&"f_denver".to_string()), "{label:?}");
    assert!(label.outcomes_reported >= 1);
    let outcomes = drain(&mut rx);
    assert!(outcomes.iter().all(|(p, b)| p == "/v1/decision/outcome" && b.to_string().contains(LABEL_SOURCE)), "{outcomes:?}");
    let conn = db.connect().unwrap();
    let unlabelled: i64 = conn.query_row("SELECT COUNT(*) FROM memory_s1_decisions WHERE observation_id = ?1 AND user_label IS NULL", params![recall_id], |r| r.get(0)).unwrap();
    assert_eq!(unlabelled, 0);
    // Labelled once only.
    assert_eq!(label_from_answer(&client, &db, "u1", "sess-1", "Denver").await.recall_id, None);
}

#[tokio::test]
async fn shadow_is_skipped_when_s1_is_off() {
    let db = db();
    seed(&db);
    let mut client = S1Client::new("http://127.0.0.1:9");
    client.enabled = false;
    assert!(shadow_recall(&client, &db, "u1", None, "rec_x", "Maya", None, &[], &RetrieveConfig::with_k(3)).await.is_none());
}

#[test]
fn answer_use_is_lexical_overlap_of_the_memory() {
    assert!(answer_uses("Maya lives in Denver these days", "Maya lives in Denver now."));
    assert!(!answer_uses("I don't know where she lives", "Maya drives a blue Subaru."));
    assert!(answer_uses("Biscuit", "Biscuit"));
}

#[test]
fn fact_listings_carry_memory_type() {
    let db = db();
    fact(&db, "f1", "Maya prefers tea.");
    crate::memory_relations::set_fact_type(&db.connect().unwrap(), "u1", "f1", crate::memory_relations::MemoryType::Preference).unwrap();
    let facts = kernel::list_facts(&db, "u1", None, 10).unwrap();
    assert_eq!(facts[0].memory_type.as_deref(), Some("preference"));
    assert_eq!(serde_json::to_value(&facts[0]).unwrap()["memory_type"], "preference");
}

const STOP_FOR_MOCK: &[&str] = &["maya", "maya's"];

/// Keyword "S1" for the eval test: relevant on any shared content token.
fn keyword_brain() -> Brain {
    Arc::new(|b, s| {
        if b == BANK_ROUTE {
            return ("all".into(), 0.9);
        }
        let (q, m) = s.split_once("\n\nmemory").unwrap_or((s, ""));
        let qt: HashSet<String> = tokens(q).into_iter().filter(|t| !STOP_FOR_MOCK.contains(&t.as_str()) && t != "question").collect();
        let hit = tokens(memory_of(m)).iter().filter(|t| qt.contains(*t)).count();
        let ans = match (b, hit) {
            (BANK_IS_RELEVANT, 0) => "not_relevant",
            (BANK_IS_RELEVANT, _) => "relevant",
            (_, 0) => "low",
            (_, 1) => "medium",
            _ => "high",
        };
        (ans.into(), 0.9)
    })
}

#[tokio::test]
async fn eval_harness_runs_on_the_tiny_fixture() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/tiny_locomo.json")).unwrap();
    let (url, _rx) = mock_s1(keyword_brain()).await;
    let report = run_eval(&db(), &fixture, &S1Client::new(&url), &EmbedClient::new(None, "hash"), 5).await.unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report.questions, 6);
    for arm in [&report.incumbent, &report.pipeline] {
        assert!((0.0..=1.0).contains(&arm.recall_at_k) && (0.0..=1.0).contains(&arm.evidence_precision));
    }
    assert!(report.pipeline.recall_at_k > 0.0);
    // The superseded Austin fact never comes back. (The tires fact reaches
    // RANK by expansion; the keyword mock scores it low, which is RANK working.)
    assert!(report.per_question.iter().all(|q| !q.pipeline.contains(&"F1".to_string())));
    let car = report.per_question.iter().find(|q| q.question.contains("car")).unwrap();
    assert!(car.pipeline.contains(&"F4".to_string()), "{car:?}");
    // S1 off: the S0-only pipeline still runs.
    let mut off = S1Client::new("http://127.0.0.1:9");
    off.enabled = false;
    let r = run_eval(&db(), &fixture, &off, &EmbedClient::new(None, "hash"), 5).await.unwrap();
    assert_eq!(r.questions, 6);
}

/// Real run: `ALLTERNIT_MEMORY_EVAL_FIXTURE=<fixture.json>` (and optionally
/// `ALLTERNIT_S1_URL`, `ALLTERNIT_EMBED_URL`, `ALLTERNIT_MEMORY_EVAL_K`);
/// writes the report to `ALLTERNIT_MEMORY_EVAL_OUT` (default stdout).
#[tokio::test]
#[ignore]
async fn memory_eval_fixture() {
    let path = std::env::var("ALLTERNIT_MEMORY_EVAL_FIXTURE").expect("ALLTERNIT_MEMORY_EVAL_FIXTURE");
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let k = std::env::var("ALLTERNIT_MEMORY_EVAL_K").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    let mut s1 = S1Client::from_env();
    s1.reporter.timeout = std::time::Duration::from_secs(30);
    let report = run_eval(&db(), &fixture, &s1, &EmbedClient::from_env(), k).await.unwrap();
    let mut summary = serde_json::to_value(&report).unwrap();
    let json = serde_json::to_string_pretty(&report).unwrap();
    match std::env::var("ALLTERNIT_MEMORY_EVAL_OUT") {
        Ok(out) => std::fs::write(out, &json).unwrap(),
        Err(_) => println!("{json}"),
    }
    summary["per_question"] = Value::Null;
    eprintln!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
