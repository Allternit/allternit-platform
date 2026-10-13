use super::*;
use crate::db::DbHandle;

fn test_db() -> DbHandle {
    std::env::set_var("ALLTERNIT_EMBED_URL", "off");
    DbHandle::new(std::env::temp_dir().join(format!("allternit-entity-pages-{}.db", uuid::Uuid::new_v4()))).unwrap()
}

fn fact(conn: &Connection, id: &str, user: &str, text: &str) {
    conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, ?2, ?3, 0.9)", params![id, user, text]).unwrap();
}

fn doc(conn: &Connection, id: &str, user: &str, title: &str, content: &str) {
    conn.execute("INSERT INTO memory_documents (id, user_id, title, content) VALUES (?1, ?2, ?3, ?4)", params![id, user, title, content])
        .unwrap();
}

/// Run passes until the queue is empty; the summed report.
fn drain(conn: &Connection, batch: usize) -> PassReport {
    let mut total = PassReport::default();
    loop {
        let r = run_pass(conn, batch).unwrap();
        if r.targets == 0 {
            return total;
        }
        total.targets += r.targets;
        total.changed += r.changed;
        total.pages_written += r.pages_written;
        total.pages_deleted += r.pages_deleted;
    }
}

fn key_entity(conn: &Connection, user: &str, key: &str) -> Option<String> {
    conn.query_row("SELECT entity_id FROM memory_entity_keys WHERE user_id = ?1 AND key = ?2", params![user, key], |r| r.get(0)).optional().unwrap()
}

fn page_of(conn: &Connection, user: &str, key: &str) -> Option<EntityPage> {
    key_entity(conn, user, key).and_then(|id| load_page(conn, user, &id).unwrap())
}

fn all_pages(conn: &Connection, user: &str) -> BTreeMap<String, String> {
    let mut stmt = conn.prepare("SELECT id, page_json FROM memory_entity_pages WHERE user_id = ?1").unwrap();
    let rows = stmt.query_map(params![user], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

#[test]
fn extracts_names_structured_ids_and_alias_evidence() {
    let ex = extract(
        "We met Dana Whitfield at the Allternit Platform (AP) offsite. Please ping @dana_w or dana@acme.io, \
         see https://www.acme.io/docs/ and `memory_index::hybrid_search`. Acme Corp, aka ACME, signed on Monday.",
        &HashSet::new(),
    );
    let keys: BTreeSet<&str> = ex.mentions.iter().map(|m| m.key.as_str()).collect();
    for k in ["danawhitfield", "allternitplatform", "ap", "@dana_w", "mailto:dana@acme.io", "url:acme.io/docs", "code:memory_index::hybrid_search", "acmecorp", "acme"] {
        assert!(keys.contains(k), "missing {k}: {keys:?}");
    }
    for k in ["we", "please", "monday", "danaw", "ping"] {
        assert!(!keys.contains(k), "unexpected {k}: {keys:?}");
    }
    assert!(ex.links.contains(&("allternitplatform".into(), "ap".into())), "{:?}", ex.links);
    assert!(ex.links.contains(&("acme".into(), "acmecorp".into())), "{:?}", ex.links);
    // Initials must match for an acronym alias.
    let ex = extract("They joined Northwind Traders (XYZ) last year.", &HashSet::new());
    assert!(ex.links.is_empty(), "{:?}", ex.links);
    // Seeds match in any case when multi-word; possessives normalize.
    let seeds: HashSet<String> = ["projectapollo".to_string()].into();
    let ex = extract("we should revisit project apollo's budget", &seeds);
    assert_eq!(ex.mentions.len(), 1);
    assert_eq!((ex.mentions[0].key.as_str(), ex.mentions[0].surface.as_str()), ("projectapollo", "project apollo"));
    assert_eq!(name_key("The ACME Corp.'s"), name_key("acme-corp"));
}

#[test]
fn resolution_merges_aliases_and_spelling_variants() {
    let db = test_db();
    let conn = db.connect().unwrap();
    doc(&conn, "d1", "alice", "Kickoff notes", "The Allternit Platform (AP) launches in spring. Dana Whitfield owns it.");
    fact(&conn, "f1", "alice", "AP pricing is per seat.");
    fact(&conn, "f2", "alice", "Dana Whitfield's team writes the launch docs.");
    fact(&conn, "f3", "alice", "Contract with Acme Corp. renews next spring.");
    fact(&conn, "f4", "alice", "The Acme-Corp invoice arrived late.");
    fact(&conn, "f5", "alice", "Solo Mention appears once.");
    drain(&conn, 100);

    let ap = page_of(&conn, "alice", "ap").expect("AP page");
    assert_eq!(Some(&ap.id), key_entity(&conn, "alice", "allternitplatform").as_ref(), "AP and Allternit Platform are one entity");
    assert_eq!(ap.name, "Allternit Platform");
    assert!(ap.aliases.contains(&"AP".to_string()), "{:?}", ap.aliases);
    assert_eq!(ap.target_count, 2);
    assert_eq!(ap.documents[0].id, "d1");
    assert!(ap.documents[0].text.contains("launches in spring"), "{:?}", ap.documents[0]);
    assert_eq!(ap.facts.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["f1"]);

    let acme = page_of(&conn, "alice", "acmecorp").expect("Acme Corp page");
    assert_eq!(acme.target_count, 2, "Acme Corp. and Acme-Corp normalize to one key");
    let dana = page_of(&conn, "alice", "danawhitfield").expect("Dana page");
    assert_eq!(dana.name, "Dana Whitfield", "possessive stripped");
    assert!(dana.related.iter().any(|r| r.id == ap.id), "co-mentioned in d1: {:?}", dana.related);
    assert!(page_of(&conn, "alice", "solomention").is_none(), "one mention is not a page");
    assert!(key_entity(&conn, "alice", "solomention").is_some(), "but it is resolved, ready for a second mention");
}

#[test]
fn resolution_never_crosses_users() {
    let db = test_db();
    let conn = db.connect().unwrap();
    // Bob states the alias; Alice never does.
    fact(&conn, "b1", "bob", "Bob works at the Allternit Platform (AP) office.");
    fact(&conn, "b2", "bob", "AP badges expire yearly.");
    fact(&conn, "a1", "alice", "AP stands for accounts payable here.");
    fact(&conn, "a2", "alice", "AP closes the books on the 5th.");
    fact(&conn, "a3", "alice", "Allternit Platform hosts the wiki.");
    fact(&conn, "a4", "alice", "Allternit Platform uptime was fine.");
    drain(&conn, 100);

    let (bob_ap, bob_platform) = (key_entity(&conn, "bob", "ap").unwrap(), key_entity(&conn, "bob", "allternitplatform").unwrap());
    assert_eq!(bob_ap, bob_platform);
    let (alice_ap, alice_platform) = (key_entity(&conn, "alice", "ap").unwrap(), key_entity(&conn, "alice", "allternitplatform").unwrap());
    assert_ne!(alice_ap, alice_platform, "Bob's alias evidence must not merge Alice's entities");
    assert_ne!(alice_ap, bob_ap, "entity ids are per user");
    for (user, own) in [("alice", ["a1", "a2", "a3", "a4"]), ("bob", ["b1", "b2", "b1", "b2"])] {
        for (_, json) in all_pages(&conn, user) {
            let p: EntityPage = serde_json::from_str(&json).unwrap();
            assert!(p.facts.iter().all(|f| own.contains(&f.id.as_str())), "{user} sees {:?}", p.facts);
        }
    }
    assert!(search_pages(&conn, "alice", Some("AP"), 10).unwrap().iter().all(|p| p.id != bob_ap));
    assert!(load_page(&conn, "alice", &bob_ap).unwrap().is_none());
    assert!(mention_links(&conn, "alice", &bob_ap, 10).unwrap().is_empty());
}

#[test]
fn incremental_pass_touches_only_changed_targets_and_pages() {
    let db = test_db();
    let conn = db.connect().unwrap();
    fact(&conn, "f1", "alice", "Dana Whitfield leads Project Falcon.");
    fact(&conn, "f2", "alice", "Project Falcon ships in June.");
    fact(&conn, "f3", "alice", "Dana Whitfield prefers async updates.");
    fact(&conn, "f4", "alice", "Marco Ruiz reviews the Orbit Ledger.");
    fact(&conn, "f5", "alice", "Orbit Ledger closes at month end.");
    let first = drain(&conn, 100);
    assert_eq!(first.targets, 5);
    assert!(page_of(&conn, "alice", "marcoruiz").is_none());
    conn.execute("UPDATE memory_entity_pages SET updated_at = '2000-01-01'", []).unwrap();
    let stamp = |key: &str| -> String {
        conn.query_row(
            "SELECT p.updated_at FROM memory_entity_pages p JOIN memory_entity_keys k ON k.entity_id = p.id WHERE k.user_id = 'alice' AND k.key = ?1",
            params![key],
            |r| r.get(0),
        )
        .unwrap()
    };

    // No-op update: read again, nothing changes.
    conn.execute("UPDATE memory_facts SET fact = fact WHERE id = 'f5'", []).unwrap();
    let r = run_pass(&conn, 100).unwrap();
    assert_eq!((r.targets, r.changed, r.pages_written, r.pages_deleted), (1, 0, 0, 0));

    conn.execute("UPDATE memory_facts SET fact = 'Project Falcon ships in July with Marco Ruiz.' WHERE id = 'f2'", []).unwrap();
    let r = run_pass(&conn, 100).unwrap();
    assert_eq!((r.targets, r.changed), (1, 1), "only the edited fact is re-read");
    let falcon = page_of(&conn, "alice", "projectfalcon").unwrap();
    assert!(falcon.facts.iter().any(|f| f.text.contains("July")));
    assert!(page_of(&conn, "alice", "marcoruiz").is_some(), "Marco now has two targets");
    assert_ne!(stamp("projectfalcon"), "2000-01-01");
    // Marco became a page, so the Orbit Ledger page lists him as related.
    let orbit = page_of(&conn, "alice", "orbitledger").unwrap();
    assert!(orbit.related.iter().any(|x| x.name == "Marco Ruiz"), "{:?}", orbit.related);
    assert_eq!(stamp("danawhitfield"), "2000-01-01", "an unaffected page is not rewritten");

    // Deleting a target removes its mentions; Marco drops back below the bar.
    conn.execute("DELETE FROM memory_facts WHERE id = 'f2'", []).unwrap();
    let r = run_pass(&conn, 100).unwrap();
    assert_eq!(r.targets, 1);
    assert!(page_of(&conn, "alice", "marcoruiz").is_none());
    assert!(page_of(&conn, "alice", "projectfalcon").is_none(), "one remaining target");
}

#[test]
fn pages_are_deterministic_across_order_and_batching() {
    let items: Vec<(&str, &str)> = vec![
        ("f1", "Dana Whitfield leads Project Falcon at Northwind Traders (NT)."),
        ("f2", "NT signed the Project Falcon contract."),
        ("f3", "Dana Whitfield uses dana@northwind.example for invoices."),
        ("f4", "Send invoices to dana@northwind.example by Friday."),
        ("f5", "Project Falcon tracks work in `falcon_tracker`."),
        ("f6", "The falcon_tracker board lives at https://boards.example/falcon."),
    ];
    let build = |order: Vec<usize>, batch: usize| {
        let db = test_db();
        let conn = db.connect().unwrap();
        for i in order {
            fact(&conn, items[i].0, "alice", items[i].1);
            // Interleave passes with writes.
            run_pass(&conn, batch).unwrap();
        }
        drain(&conn, batch);
        (db, all_pages(&conn, "alice"))
    };
    let (db_a, a) = build((0..items.len()).collect(), 1000);
    let (_db_b, b) = build((0..items.len()).rev().collect(), 1);
    assert!(!a.is_empty());
    assert_eq!(a, b, "same memory, same pages, whatever the order");
    let conn = db_a.connect().unwrap();
    let nt = page_of(&conn, "alice", "nt").unwrap();
    assert_eq!(nt.name, "Northwind Traders");
    assert!(page_of(&conn, "alice", "mailto:dana@northwind.example").is_some());
    assert!(page_of(&conn, "alice", "code:falcon_tracker").is_some());
    // A full rebuild reproduces every page byte for byte.
    let r = rebuild_all(&conn, "alice").unwrap();
    assert_eq!((r.pages_written, r.pages_deleted), (0, 0));
    assert_eq!(all_pages(&conn, "alice"), a);
}

#[test]
fn seed_entities_name_pages_and_pull_in_older_lowercase_mentions() {
    let db = test_db();
    let conn = db.connect().unwrap();
    doc(&conn, "d1", "alice", "Planning", "we should revisit project apollo budgets before the review.");
    fact(&conn, "f1", "alice", "The budget for project apollo is fixed.");
    crate::memory_index::chunk_pending_documents(&conn, 10).unwrap();
    drain(&conn, 100);
    assert!(key_entity(&conn, "alice", "projectapollo").is_none());

    conn.execute(
        "INSERT INTO memory_entities (id, user_id, entity_id, name, type, properties) VALUES ('me1', 'alice', 'e1', 'Project Apollo', 'Project', ?1)",
        params![r#"{"aliases": ["Apollo"]}"#],
    )
    .unwrap();
    fact(&conn, "f2", "alice", "Apollo slipped a week.");
    drain(&conn, 100);
    let p = page_of(&conn, "alice", "projectapollo").expect("seed page");
    assert_eq!((p.name.as_str(), p.kind.as_str(), p.memory_entity_id.as_deref()), ("Project Apollo", "Project", Some("me1")));
    assert_eq!(p.documents.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d1"], "older document re-read once the seed exists");
    let mut facts: Vec<&str> = p.facts.iter().map(|f| f.id.as_str()).collect();
    facts.sort();
    assert_eq!(facts, ["f1", "f2"], "the alias resolves to the seed");
    assert_eq!(Some(p.id.clone()), key_entity(&conn, "alice", "apollo"));
    let text = render_text(&p);
    assert!(text.starts_with("# Project Apollo\n") && text.contains("[document:d1]"), "{text}");
}

#[test]
fn text_lookup_finds_pages_named_in_a_goal() {
    let db = test_db();
    let conn = db.connect().unwrap();
    fact(&conn, "f1", "alice", "Report covers Project Falcon only.");
    fact(&conn, "f2", "alice", "Project Falcon ships in June.");
    fact(&conn, "f3", "alice", "Report drafts go to Marco Ruiz.");
    fact(&conn, "f4", "alice", "Marco Ruiz likes tables.");
    drain(&conn, 100);
    let pages = pages_for_text(&conn, "alice", "draft the project falcon report for Marco Ruiz", 2).unwrap();
    assert_eq!(pages.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Project Falcon", "Marco Ruiz"]);
    // "report" was capitalized twice (a page), but a one-word lowercase goal word is not a lookup.
    assert!(page_of(&conn, "alice", "report").is_some());
    assert!(pages_for_text(&conn, "alice", "write a report", 2).unwrap().is_empty());
    assert!(pages_for_text(&conn, "bob", "draft the Project Falcon report", 2).unwrap().is_empty());
}

#[tokio::test]
async fn routes_scope_to_the_authenticated_user() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let dir = std::env::temp_dir().join(format!("allternit-entity-pages-routes-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("ALLTERNIT_EMBED_URL", "off");
    let state = crate::test_helpers::app_state(&dir).await;
    {
        let conn = state.db.connect().unwrap();
        fact(&conn, "f1", "alice", "Dana Whitfield leads Project Falcon.");
        fact(&conn, "f2", "alice", "Project Falcon ships in June.");
        drain(&conn, 100);
    }
    let id = key_entity(&state.db.connect().unwrap(), "alice", "projectfalcon").unwrap();
    let user = |id: &str| AuthUser {
        user_id: id.into(),
        email: None,
        name: None,
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    };
    let call = |u: &str, path: String| {
        let app = memory_entity_pages_router().merge(crate::memory_routes::memory_router()).layer(axum::Extension(user(u))).with_state(state.clone());
        let u = u.to_string();
        async move {
            let resp = app.oneshot(Request::get(path).header("x-allternit-user-id", u).body(Body::empty()).unwrap()).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
        }
    };
    let (s, v) = call("alice", format!("/memory/entities/{id}")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["page"]["name"], "Project Falcon");
    assert_eq!(v["mentions"].as_array().unwrap().len(), 2);
    assert!(v["text"].as_str().unwrap().starts_with("# Project Falcon"));
    let (s, _) = call("bob", format!("/memory/entities/{id}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "another user's page is not found");

    let (s, v) = call("alice", "/memory/entities?q=falcon".into()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["pages"][0]["id"], id.as_str());
    assert!(v["entities"].is_array(), "legacy field kept");
    let (_, v) = call("bob", "/memory/entities?q=falcon".into()).await;
    assert_eq!(v["pages"].as_array().unwrap().len(), 0);
}
