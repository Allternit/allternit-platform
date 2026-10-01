use super::*;
use std::sync::Mutex as StdMutex;

fn db() -> DbHandle {
    let dir = std::env::temp_dir().join(format!("mem-cons-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    DbHandle::new(dir.join("t.db")).unwrap()
}

fn fact(conn: &Connection, id: &str, text: &str, conf: f64, age_days: i64) {
    conn.execute(
        "INSERT INTO memory_facts (id, user_id, fact, confidence, valid_from) VALUES (?1, 'u1', ?2, ?3, datetime('now', ?4))",
        params![id, text, conf, format!("-{age_days} days")],
    )
    .unwrap();
}

fn valid(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT valid_until IS NULL FROM memory_facts WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
}

/// Off client: no S1 runtime (the incumbent decides).
fn off() -> S1Client {
    let mut c = S1Client::new("http://127.0.0.1:9");
    c.enabled = false;
    c
}

/// Mock S1 runtime: answers every RELATION decision with `answer`, records
/// reported outcomes.
async fn spawn_s1(answer: &'static str, confidence: f64) -> (String, Arc<StdMutex<Vec<String>>>) {
    let outcomes = Arc::new(StdMutex::new(Vec::new()));
    let o2 = outcomes.clone();
    let app = Router::new()
        .route(
            "/v1/decision",
            post(move || async move { Json(json!({ "answer": { "candidate_id": answer }, "confidence": confidence, "extensions": { "x-decision_id": format!("d_{}", Uuid::new_v4().simple()) } })) }),
        )
        .route(
            "/v1/decision/outcome",
            post(move |Json(b): Json<serde_json::Value>| {
                let o = o2.clone();
                async move {
                    o.lock().unwrap().push(b.to_string());
                    Json(json!({ "ok": true }))
                }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), outcomes)
}

#[test]
fn pairs_exact_edge_and_near_duplicates_once_each() {
    let mk = |id: &str, t: &str, c: f64| FactRow { id: id.into(), agent_id: None, fact: t.into(), confidence: c, valid_from: "2026-01-01".into(), retrieval_count: 0 };
    let facts = vec![
        mk("a", "Lives in Austin, Texas.", 0.7),
        mk("b", "lives in austin texas", 0.9),
        mk("c", "Prefers dark roast coffee", 0.8),
        mk("d", "Prefers dark roast coffee beans", 0.8),
        mk("e", "Drives a Tesla", 0.8),
        mk("f", "Owns an EV", 0.8),
    ];
    let mut edges = HashSet::new();
    edges.insert(("e".to_string(), "f".to_string()));
    edges.insert(("f".to_string(), "e".to_string()));
    let pairs = candidate_pairs(&facts, &edges);
    assert_eq!(pairs.len(), 3);
    // Exact: higher confidence survives.
    assert_eq!((pairs[0].keep.id.as_str(), pairs[0].drop.id.as_str(), pairs[0].incumbent_same), ("b", "a", true));
    // Near (Jaccard 0.8): S1-only candidate, incumbent says keep both.
    assert!(!pairs[1].incumbent_same && pairs[1].jaccard >= NEAR_JACCARD);
    // Earlier `same` edge: incumbent merges.
    assert!(pairs[2].incumbent_same);
}

#[tokio::test]
async fn incumbent_merges_exact_duplicates_idempotently() {
    let db = db();
    {
        let conn = db.connect().unwrap();
        fact(&conn, "f1", "Works at Acme Corp", 0.8, 5);
        fact(&conn, "f2", "works at acme corp.", 0.6, 1);
        fact(&conn, "f3", "Works at Acme Corp as an engineer", 0.8, 1);
        conn.execute(
            "INSERT INTO memory_adapter_links (user_id, source, external_id, fact_id, content_hash) VALUES ('u1', 'gizzi.memdir', 'x.md', 'f2', 'h')",
            [],
        )
        .unwrap();
    }
    let r = run_for_user(&db, &off(), "u1", false).await.unwrap();
    assert_eq!((r.scanned, r.merged, r.shadow_decisions), (3, 1, 0));
    let conn = db.connect().unwrap();
    assert!(valid(&conn, "f1") && !valid(&conn, "f2") && valid(&conn, "f3"));
    let edge: String = conn
        .query_row("SELECT origin FROM memory_relationships WHERE source_entity_id = 'f2' AND target_entity_id = 'f1' AND relation_type = 'same'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(edge, rel::ORIGIN_INCUMBENT);
    // The adapter item follows its fact to the survivor; nothing was deleted.
    let linked: String = conn.query_row("SELECT fact_id FROM memory_adapter_links WHERE external_id = 'x.md'", [], |r| r.get(0)).unwrap();
    assert_eq!(linked, "f1");
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM memory_facts", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 3);
    drop(conn);
    // Second run: nothing left to merge.
    let r2 = run_for_user(&db, &off(), "u1", false).await.unwrap();
    assert_eq!(r2.merged, 0);
    let runs: i64 = db.connect().unwrap().query_row("SELECT COUNT(*) FROM memory_consolidation_runs WHERE finished_at IS NOT NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(runs, 2);
}

#[tokio::test]
async fn s1_shadow_never_changes_storage_until_live() {
    let db = db();
    {
        let conn = db.connect().unwrap();
        fact(&conn, "c1", "Prefers dark roast coffee", 0.8, 3);
        fact(&conn, "c2", "Prefers dark roast coffee beans", 0.8, 2);
    }
    let (url, outcomes) = spawn_s1("same", 0.95).await;
    let client = S1Client::new(&url);
    // Shadow: S1 says same, the incumbent (texts differ) keeps both.
    let r = run_for_user(&db, &client, "u1", false).await.unwrap();
    assert_eq!((r.merged, r.shadow_decisions, r.outcomes_reported), (0, 1, 1));
    assert!(outcomes.lock().unwrap()[0].contains("unrelated"));
    {
        let conn = db.connect().unwrap();
        assert!(valid(&conn, "c1") && valid(&conn, "c2"));
        let (bank, label): (String, String) = conn
            .query_row("SELECT bank, incumbent_label FROM memory_s1_decisions WHERE candidate_fact_id = 'c2'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((bank.as_str(), label.as_str()), (BANK_RELATION, "unrelated"));
    }
    // Live (Q26 passed): S1's confident `same` merges.
    let r = run_for_user(&db, &client, "u1", true).await.unwrap();
    assert_eq!(r.merged, 1);
    let conn = db.connect().unwrap();
    assert!(!valid(&conn, "c2"));
    let origin: String = conn.query_row("SELECT origin FROM memory_relationships WHERE source_entity_id = 'c2'", [], |r| r.get(0)).unwrap();
    assert_eq!(origin, "s1");
}

#[tokio::test]
async fn live_s1_falls_back_to_the_incumbent_when_unreachable() {
    let db = db();
    {
        let conn = db.connect().unwrap();
        fact(&conn, "a", "Allergic to peanuts", 0.8, 3);
        fact(&conn, "b", "allergic to peanuts", 0.8, 2);
    }
    let r = run_for_user(&db, &off(), "u1", true).await.unwrap();
    assert_eq!(r.merged, 1);
}

#[tokio::test]
async fn decay_marks_only_stale_unretrieved_low_confidence_and_recall_revives() {
    let db = db();
    {
        let conn = db.connect().unwrap();
        fact(&conn, "old_low", "Maybe likes jazz", 0.3, 200);
        fact(&conn, "old_high", "Birthday is in May", 0.9, 200);
        fact(&conn, "new_low", "Might visit Lisbon", 0.3, 5);
        fact(&conn, "old_used", "Possibly vegetarian", 0.3, 200);
        conn.execute("UPDATE memory_facts SET retrieval_count = 2 WHERE id = 'old_used'", []).unwrap();
    }
    let r = run_for_user(&db, &off(), "u1", false).await.unwrap();
    assert_eq!(r.decayed, 1);
    let conn = db.connect().unwrap();
    let decayed: Vec<String> = conn
        .prepare("SELECT id FROM memory_facts WHERE decayed_at IS NOT NULL")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(decayed, vec!["old_low".to_string()]);
    assert!(valid(&conn, "old_low"), "decay never retires or deletes");
    // A recall that returns it revives it.
    let hits = crate::memory_kernel_service::recall_with_embedding(&db, "u1", None, None, "jazz", None, 5).unwrap();
    assert!(hits.iter().any(|h| h.id == "old_low"));
    let (count, decayed_at): (i64, Option<String>) =
        conn.query_row("SELECT retrieval_count, decayed_at FROM memory_facts WHERE id = 'old_low'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((count, decayed_at), (1, None));
}

#[test]
fn adapter_upsert_is_idempotent_and_updates_supersede() {
    let db = db();
    let conn = db.connect().unwrap();
    let item = |t: &str| AdapterItem { external_id: "user_role.md".into(), text: t.into(), memory_type: Some("preference".into()), agent_id: None, confidence: None };
    let r = adapter_upsert(&conn, "u1", "gizzi.memdir", &[item("Eoj prefers terse answers")]).unwrap();
    assert_eq!((r.created, r.updated, r.unchanged), (1, 0, 0));
    let first = r.fact_ids[0].clone();
    // Same content again (one-time import re-run): no new fact.
    let r = adapter_upsert(&conn, "u1", "gizzi.memdir", &[item("Eoj prefers terse answers")]).unwrap();
    assert_eq!((r.created, r.unchanged), (0, 1));
    // Changed content: a new fact that updates (soft-supersedes) the old one.
    let r = adapter_upsert(&conn, "u1", "gizzi.memdir", &[item("Eoj prefers terse answers with links")]).unwrap();
    assert_eq!(r.updated, 1);
    assert!(!valid(&conn, &first) && valid(&conn, &r.fact_ids[0]));
    let mtype: String = conn.query_row("SELECT memory_type FROM memory_facts WHERE id = ?1", params![r.fact_ids[0]], |r| r.get(0)).unwrap();
    assert_eq!(mtype, "preference");
    // Delete retires the fact (soft) and drops the link.
    assert_eq!(adapter_delete(&conn, "u1", "gizzi.memdir", &["user_role.md".into()]).unwrap(), 1);
    assert!(!valid(&conn, &r.fact_ids[0]));
    // Other users can't touch it.
    assert_eq!(adapter_delete(&conn, "u2", "gizzi.memdir", &["user_role.md".into()]).unwrap(), 0);
}

#[test]
fn due_users_skips_recent_runs() {
    let db = db();
    let conn = db.connect().unwrap();
    fact(&conn, "a", "x", 0.8, 1);
    conn.execute("INSERT INTO memory_facts (id, user_id, fact) VALUES ('b', 'u2', 'y')", []).unwrap();
    conn.execute("INSERT INTO memory_consolidation_runs (id, user_id) VALUES ('r', 'u2')", []).unwrap();
    assert_eq!(due_users(&conn, 10).unwrap(), vec!["u1".to_string()]);
}
