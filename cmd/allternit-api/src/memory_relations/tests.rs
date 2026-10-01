use super::*;
use crate::memory_extraction::{apply_ops_typed, relation_candidates, MemoryOp};
use crate::memory_kernel_service as kernel;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn db() -> DbHandle {
    DbHandle::new_memory().expect("memory db")
}

fn obs(db: &DbHandle, text: &str) -> String {
    kernel::record_observation(db, "u1", None, None, "turn_user", text, Some("user")).unwrap()
}

fn add(fact: &str, t: Option<&str>) -> MemoryOp {
    MemoryOp::Add { fact: fact.into(), memory_type: t.map(str::to_string) }
}

/// Mock decision runtime: `/v1/decision` answers `answer` with ids dec-1,
/// dec-2, …; `/v1/decision/outcome` answers {}. Every request is sent as
/// "path|body".
async fn mock_s1(answer: &'static str) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let n = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let (tx, n) = (tx.clone(), n.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let mut got = Vec::new();
                let (path, body) = loop {
                    let r = s.read(&mut buf).await.unwrap_or(0);
                    if r == 0 { return; }
                    got.extend_from_slice(&buf[..r]);
                    let txt = String::from_utf8_lossy(&got).to_string();
                    if let Some(i) = txt.find("\r\n\r\n") {
                        let len = txt.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if got.len() >= i + 4 + len {
                            let path = txt.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
                            break (path, txt[i + 4..].to_string());
                        }
                    }
                };
                let resp = if path == "/v1/decision" {
                    let id = n.fetch_add(1, Ordering::SeqCst) + 1;
                    json!({ "answer": answer, "confidence": 0.61, "extensions": { "x-decision_id": format!("dec-{id}") } }).to_string()
                } else {
                    "{}".to_string()
                };
                let _ = tx.send(format!("{path}|{body}"));
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{resp}", resp.len()).as_bytes()).await;
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

#[test]
fn closed_sets_round_trip_and_the_schema_enforces_them() {
    for t in MemoryType::ALL {
        assert_eq!(MemoryType::parse(t.as_str()), Some(*t));
    }
    for r in RelationType::ALL {
        assert_eq!(RelationType::parse(r.as_str()), Some(*r));
    }
    assert_eq!(MemoryType::parse("Task-State"), Some(MemoryType::TaskState));
    assert_eq!(RelationType::parse("caused-by"), Some(RelationType::CausedBy));
    assert_eq!(MemoryType::ALL.len(), 8);
    assert_eq!(RelationType::ALL.len(), 9);
    assert!(MemoryType::parse("opinion").is_none());

    let db = db();
    let o = obs(&db, "x");
    let f = kernel::persist_facts(&db, "u1", None, &o, &["User uses Rust daily.".into()]).unwrap();
    let conn = db.connect().unwrap();
    assert!(conn.execute("UPDATE memory_facts SET memory_type = 'opinion' WHERE id = ?1", params![f[0].id]).is_err());
    assert!(conn
        .execute("INSERT INTO memory_relationships (id, user_id, source_entity_id, target_entity_id, relation, relation_type) VALUES ('r','u1','a','b','x','likes')", [])
        .is_err());
}

#[test]
fn write_relation_fills_both_edge_tables_and_supersedes() {
    let db = db();
    let o = obs(&db, "x");
    let f = kernel::persist_facts(&db, "u1", None, &o, &["User lives in Austin.".into(), "User lives in Denver.".into()]).unwrap();
    let conn = db.connect().unwrap();
    let id = write_relation(&conn, "u1", (NodeKind::Fact, &f[1].id), RelationType::Updates, (NodeKind::Fact, &f[0].id), 0.9, ORIGIN_INCUMBENT, Some("dec-x")).unwrap();
    let (rt, sk, tk, origin, dec): (String, String, String, String, String) = conn
        .query_row("SELECT relation_type, source_kind, target_kind, origin, decision_id FROM memory_relationships WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .unwrap();
    assert_eq!((rt.as_str(), sk.as_str(), tk.as_str(), origin.as_str(), dec.as_str()), ("updates", "fact", "fact", "incumbent_llm", "dec-x"));
    let (rel, meta): (String, String) = conn
        .query_row("SELECT relationship, metadata FROM memory_edges WHERE source = ?1 AND target = ?2", params![f[1].id, f[0].id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(rel, "updates");
    assert!(meta.contains(&id));
    // Soft supersession: the old fact is out of recall but still auditable.
    let current: Vec<String> = kernel::list_facts(&db, "u1", None, 10).unwrap().into_iter().map(|f| f.id).collect();
    assert_eq!(current, vec![f[1].id.clone()]);
    let until: Option<String> = conn.query_row("SELECT valid_until FROM memory_facts WHERE id = ?1", params![f[0].id], |r| r.get(0)).unwrap();
    assert!(until.is_some());
    // A non-superseding relation leaves the target alone.
    write_relation(&conn, "u1", (NodeKind::Fact, &f[1].id), RelationType::AboutEntity, (NodeKind::Entity, "ent_1"), 0.9, ORIGIN_INCUMBENT, None).unwrap();
    assert_eq!(relations_of(&conn, "u1", &f[1].id).unwrap().len(), 2);
}

#[test]
fn incumbent_ops_write_types_edges_and_labels() {
    let db = db();
    let o0 = obs(&db, "seed");
    let austin = kernel::persist_facts(&db, "u1", None, &o0, &["User lives in Austin.".into()]).unwrap().remove(0);
    let linear = kernel::persist_facts(&db, "u1", None, &o0, &["User uses Linear.".into()]).unwrap().remove(0);
    let tea = kernel::persist_facts(&db, "u1", None, &o0, &["User prefers tea.".into()]).unwrap().remove(0);
    let shown: Vec<(String, String)> = vec![(austin.id.clone(), austin.fact.clone()), (linear.id.clone(), linear.fact.clone()), (tea.id.clone(), tea.fact.clone())];
    let o = obs(&db, "I moved to Denver, dropped Linear, and yes I still prefer tea.");
    let ops = vec![
        MemoryOp::Update { id: austin.id.clone(), fact: "User lives in Denver.".into(), memory_type: Some("fact".into()) },
        MemoryOp::Forget { id: linear.id.clone() },
        add("User prefers tea.", Some("preference")),
        add("User likes hiking.", Some("bogus-type")),
    ];
    let cands = relation_candidates(&db, "u1", "Denver Linear tea", &ops, &shown);
    assert_eq!(&cands[0].0, &austin.id, "touched candidates come first");
    assert_eq!(&cands[1].0, &linear.id);

    let (changed, out) = apply_ops_typed(&db, "u1", None, &o, &ops, &shown).unwrap();
    assert_eq!(changed, 4); // denver + austin retired + linear retired + hiking
    let conn = db.connect().unwrap();
    let denver = &out.produced[0];
    assert_eq!(denver.1, MemoryType::Fact);
    assert_eq!(out.produced[1].1, MemoryType::Fact, "an unknown type falls back to fact");
    assert_eq!(fact_type(&conn, "u1", &denver.0).unwrap(), Some(MemoryType::Fact));
    assert_eq!(out.observation_type, Some(MemoryType::Fact));

    assert_eq!(out.relation_label(&austin.id), (RelationType::Updates, "memory.incumbent", Some(denver.0.clone())));
    assert_eq!(out.relation_label(&linear.id).0, RelationType::Contradicts);
    assert_eq!(out.relation_label(&tea.id).0, RelationType::Same);
    assert_eq!(out.relation_label("fact_other"), (RelationType::Unrelated, "memory.incumbent.untouched", None));

    let edges = relations_of(&conn, "u1", &austin.id).unwrap();
    assert_eq!(edges, vec![("updates".into(), denver.0.clone(), austin.id.clone())]);
    assert_eq!(relations_of(&conn, "u1", &linear.id).unwrap()[0].0, "contradicts");
    assert_eq!(relations_of(&conn, "u1", &tea.id).unwrap()[0], ("same".into(), o.clone(), tea.id.clone()));
    let now: Vec<String> = kernel::list_facts(&db, "u1", None, 50).unwrap().into_iter().map(|f| f.fact).collect();
    assert!(now.contains(&"User lives in Denver.".to_string()) && !now.contains(&"User uses Linear.".to_string()));
    let ot: String = conn.query_row("SELECT memory_type FROM memory_observations WHERE id = ?1", params![o], |r| r.get(0)).unwrap();
    assert_eq!(ot, "fact");

    // No operations: the incumbent said this is not a memory.
    let o2 = obs(&db, "what is 2+2");
    let (_, out2) = apply_ops_typed(&db, "u1", None, &o2, &[], &shown).unwrap();
    assert_eq!(out2.observation_type, Some(MemoryType::NotMemory));
}

#[tokio::test]
async fn shadow_records_s1_and_reports_incumbent_labels() {
    let (url, mut rx) = mock_s1("same").await;
    let db = db();
    let o0 = obs(&db, "seed");
    let austin = kernel::persist_facts(&db, "u1", None, &o0, &["User lives in Austin.".into()]).unwrap().remove(0);
    let shown = vec![(austin.id.clone(), austin.fact.clone())];
    let o = obs(&db, "I moved to Denver.");
    let ops = vec![MemoryOp::Update { id: austin.id.clone(), fact: "User lives in Denver.".into(), memory_type: Some("fact".into()) }];
    let cands = relation_candidates(&db, "u1", "I moved to Denver.", &ops, &shown);
    let (_, out) = apply_ops_typed(&db, "u1", None, &o, &ops, &shown).unwrap();
    let denver = out.produced[0].0.clone();

    let r = shadow_turn(&S1Client::new(&url), &db, "u1", &o, "I moved to Denver.", &cands, &out).await;
    assert_eq!(r, ShadowReport { decisions: 2, outcomes_reported: 2 });

    let reqs = drain(&mut rx);
    let decisions: Vec<&Value> = reqs.iter().filter(|(p, _)| p == "/v1/decision").map(|(_, b)| b).collect();
    let outcomes: Vec<&Value> = reqs.iter().filter(|(p, _)| p == "/v1/decision/outcome").map(|(_, b)| b).collect();
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0]["backend"], "auto");
    assert_eq!(decisions[0]["request"]["decision_bank_id"], BANK_MEMORY_TYPE);
    assert_eq!(decisions[0]["request"]["extensions"]["x-motif"], "LABEL");
    assert_eq!(decisions[0]["request"]["candidates"].as_array().unwrap().len(), 9); // 8 types + unknown
    assert_eq!(decisions[1]["request"]["decision_bank_id"], BANK_RELATION);
    assert_eq!(decisions[1]["request"]["candidates"].as_array().unwrap().len(), 10);
    assert!(decisions[1]["state"].as_str().unwrap().contains("User lives in Austin."));
    // Shadow: S1 said "same" for both; the incumbent's labels are the truth.
    assert_eq!(*outcomes[0], json!({"decision_id": "dec-1", "truth": "fact", "source": "memory.incumbent"}));
    assert_eq!(*outcomes[1], json!({"decision_id": "dec-2", "truth": "updates", "source": "memory.incumbent"}));

    let conn = db.connect().unwrap();
    let rows: Vec<(String, String, Option<String>, Option<String>, String, String)> = conn
        .prepare("SELECT bank, decision_id, candidate_fact_id, produced_fact_id, s1_answer, incumbent_label FROM memory_s1_decisions ORDER BY bank")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(rows[0], ("memory.relation".into(), "dec-2".into(), Some(austin.id.clone()), Some(denver.clone()), "same".into(), "updates".into()));
    assert_eq!(rows[1], ("memory.type".into(), "dec-1".into(), None, Some(denver.clone()), "same".into(), "fact".into()));
    // S1 never changed what was stored, and the incumbent's edge carries the join key.
    let edge_dec: String = conn.query_row("SELECT decision_id FROM memory_relationships WHERE target_entity_id = ?1", params![austin.id], |r| r.get(0)).unwrap();
    assert_eq!(edge_dec, "dec-2");
    assert_eq!(kernel::list_facts(&db, "u1", None, 10).unwrap().len(), 1);

    // A user deleting the produced memory labels the MEMORY_TYPE decision.
    let ids = user_label_decisions(&conn, "u1", &denver, MemoryType::NotMemory).unwrap();
    assert_eq!(ids, vec!["dec-1".to_string()]);
    let ul: String = conn.query_row("SELECT user_label FROM memory_s1_decisions WHERE decision_id = 'dec-1'", [], |r| r.get(0)).unwrap();
    assert_eq!(ul, "not_memory");
}

#[tokio::test]
async fn shadow_is_a_noop_when_off_or_unreachable() {
    let db = db();
    let o = obs(&db, "x");
    let out = TurnOutcome { observation_type: Some(MemoryType::NotMemory), ..Default::default() };
    let mut off = S1Client::new("http://127.0.0.1:1");
    let r = shadow_turn(&off, &db, "u1", &o, "x", &[("f".into(), "y".into())], &out).await;
    assert_eq!(r, ShadowReport::default());
    off.enabled = false;
    assert_eq!(shadow_turn(&off, &db, "u1", &o, "x", &[], &out).await, ShadowReport::default());
    let n: i64 = db.connect().unwrap().query_row("SELECT COUNT(*) FROM memory_s1_decisions", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn user_edit_supersedes_with_a_typed_edge() {
    let db = db();
    let o = obs(&db, "x");
    let f = kernel::persist_facts(&db, "u1", None, &o, &["User lives in Austin.".into()]).unwrap().remove(0);
    // Type-only edit retypes in place.
    let (id, t) = edit_fact(&db, "u1", &f.id, None, Some(MemoryType::Event)).unwrap().unwrap();
    assert_eq!((id.as_str(), t), (f.id.as_str(), MemoryType::Event));
    // Text edit: new fact `updates` the old one, keeps the type.
    let (new_id, t) = edit_fact(&db, "u1", &f.id, Some("User lives in Boulder."), None).unwrap().unwrap();
    assert_ne!(new_id, f.id);
    assert_eq!(t, MemoryType::Event);
    let conn = db.connect().unwrap();
    assert_eq!(relations_of(&conn, "u1", &f.id).unwrap(), vec![("updates".into(), new_id.clone(), f.id.clone())]);
    let origin: String = conn.query_row("SELECT origin FROM memory_relationships WHERE target_entity_id = ?1", params![f.id], |r| r.get(0)).unwrap();
    assert_eq!(origin, "user");
    let now: Vec<String> = kernel::list_facts(&db, "u1", None, 10).unwrap().into_iter().map(|f| f.fact).collect();
    assert_eq!(now, vec!["User lives in Boulder.".to_string()]);
    // The superseded fact or another user's fact cannot be edited.
    assert!(edit_fact(&db, "u1", &f.id, Some("x y z w"), None).unwrap().is_none());
    assert!(edit_fact(&db, "u2", &new_id, None, Some(MemoryType::Fact)).unwrap().is_none());
}
