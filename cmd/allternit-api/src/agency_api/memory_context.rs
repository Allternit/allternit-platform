//! Memory in the Agency context step (Agency Descent, Phase 1).
//!
//! `ctx.compile_model_context` used to hand the model only a hash marker, so
//! routing's `retrieval` was trace-only. It now recalls the run owner's
//! memory through the shared memory index (`memory_index`: FTS5 + EmbeddingGemma 2
//! vectors, RRF) across every space (facts, entities, observations,
//! documents, procedures, notes) and gives the best hits to the S2/S3 steps
//! that read the context. Everything stays local: SQLite and the local
//! embedding sidecar (hash fallback when it is down).
//!
//! Three blocks, never mixed (A6): "Profile" (C3 entity pages, then the
//! documents they link), "Past conversations" (raw turns from
//! `raw_windows::search`, up to 8 dated lines, 4000 chars, none of them
//! hidden by a page) and "Other memory" (facts, documents, procedures, notes
//! from the memory index, minus what a page showed).
//!
//! `ALLTERNIT_AGENCY_MEMORY=0` turns it off; `ALLTERNIT_AGENCY_ENTITY_PAGES=0`
//! keeps recall but drops the entity pages.

use std::collections::HashSet;

use crate::db::DbHandle;
use crate::memory_entity_pages;
use crate::memory_index::{self, InputType};
use crate::memory_retrieve::raw_windows::{self, RawMode, RawQuery};
use crate::memory_retrieve::{anchors, Space};

/// Hits handed to the model, and the cap on their text.
/// Raw past turns handed to the model, and the cap on their text.
const MAX_HITS: usize = 8;
const MAX_CHARS: usize = 4000;
/// Facts/documents/notes block: its own count and cap.
const MAX_OTHER: usize = 5;
const OTHER_CHARS: usize = 1600;
/// Profile block (pages + linked documents) cap.
const PROFILE_CHARS: usize = 2400;
const PER_SPACE: usize = 3;
/// Entity pages per goal, the cap on one compact page and on all of them.
const MAX_PAGES: usize = 2;
const PAGE_CHARS: usize = 600;
pub const PAGES_CHARS: usize = 1200;
/// Documents followed from the pages (they count as hits).
pub const MAX_LINKED: usize = 3;
const LINKED_CHARS: usize = 360;

pub fn enabled() -> bool {
    std::env::var("ALLTERNIT_AGENCY_MEMORY").map(|v| v.trim() != "0").unwrap_or(true)
}

fn pages_enabled() -> bool {
    std::env::var("ALLTERNIT_AGENCY_ENTITY_PAGES").map(|v| v.trim() != "0").unwrap_or(true)
}

/// The recalled memory for `goal`: a text block (empty when nothing matched)
/// and the ids of the items in it, as `type:id` (`entity_page:id` for pages).
pub struct Recall {
    pub text: String,
    pub ids: Vec<String>,
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub async fn recall(db: &DbHandle, owner: &str, goal: &str) -> Result<Recall, String> {
    let qv = memory_index::global().embed_or_hash(&[goal.to_string()], InputType::Query).await;
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut ids = vec![];
    // (type, id) already in the profile block; documents by document id.
    let mut shown: HashSet<(String, String)> = HashSet::new();

    // Profile block: entity pages and the documents they link. It takes none
    // of the raw slots and never hides a raw hit.
    let mut profile = String::new();
    let pages = if pages_enabled() {
        memory_entity_pages::pages_for_text(&conn, owner, goal, MAX_PAGES).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "entity pages unavailable for recall");
            vec![]
        })
    } else {
        vec![]
    };
    for p in &pages {
        let (block, items) = memory_entity_pages::render_compact(p, PAGE_CHARS);
        if profile.len() + block.len() > PAGES_CHARS {
            break;
        }
        profile.push_str(&block);
        ids.push(format!("entity_page:{}", p.id));
        shown.extend(items);
    }
    let shown_pages = ids.len();
    let mut linked_lines = 0;
    let linked = pages.iter().take(shown_pages).flat_map(|p| p.documents.iter()).map(|d| ("document", d)).take(MAX_LINKED * 2);
    for (t, d) in linked {
        if linked_lines >= MAX_LINKED || !shown.insert((t.to_string(), d.id.clone())) {
            continue;
        }
        let Some((title, passage)) = memory_entity_pages::linked_passage(&conn, owner, t, &d.id, d.pos, LINKED_CHARS).map_err(|e| e.to_string())? else {
            continue;
        };
        let line = format!("- ({t}) {}: {}\n", one_line(&title), one_line(&passage));
        if profile.len() + line.len() > PROFILE_CHARS {
            break;
        }
        profile.push_str(&line);
        ids.push(format!("{t}:{}", d.id));
        linked_lines += 1;
    }

    // Raw past turns (A4/A6): only `turn_*` windows, never facts.
    let mut raw = String::new();
    let query = RawQuery { query: goal.to_string(), limit: Some(MAX_HITS), mode: RawMode::Search, ..Default::default() };
    match raw_windows::search(db, memory_index::global(), owner, query).await {
        Ok(res) => {
            for t in res.turns.iter().take(MAX_HITS) {
                let line = format!("- [{} · {}] {}\n", t.timestamp.get(..10).unwrap_or(&t.timestamp), t.speaker, one_line(&t.text));
                if raw.len() + line.len() > MAX_CHARS {
                    break;
                }
                raw.push_str(&line);
                ids.push(format!("observation:{}", t.observation_id));
            }
        }
        Err(e) => tracing::warn!(error = %e, "raw past-chat search unavailable for recall"),
    }

    // Everything else the memory index knows (facts, documents, procedures,
    // notes), in its own block so it never mixes into the raw list.
    let mut other = String::new();
    let mut hits = vec![];
    for space in Space::ALL.iter().filter(|s| !matches!(s, Space::Observations)) {
        hits.extend(anchors(&conn, *space, owner, None, goal, Some(&qv), PER_SPACE, false).map_err(|e| e.to_string())?);
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut other_lines = 0;
    for h in hits {
        if other_lines >= MAX_OTHER {
            break;
        }
        let doc = (h.item_type == "document").then(|| h.metadata["source_id"].as_str().map(str::to_string)).flatten();
        if shown.contains(&(h.item_type.clone(), h.id.clone())) || doc.is_some_and(|d| shown.contains(&("document".to_string(), d))) {
            continue;
        }
        let line = format!("- ({}) {}\n", h.item_type, one_line(&h.content));
        if other.len() + line.len() > OTHER_CHARS {
            break;
        }
        other.push_str(&line);
        ids.push(format!("{}:{}", h.item_type, h.id));
        other_lines += 1;
    }

    let mut text = String::new();
    for (head, body) in [("Profile", &profile), ("Past conversations", &raw), ("Other memory", &other)] {
        if !body.is_empty() {
            text.push_str(&format!("{head}\n{body}"));
        }
    }
    Ok(Recall { text, ids })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[tokio::test]
    async fn recalls_the_owners_relevant_memory_only() {
        std::env::set_var("ALLTERNIT_EMBED_URL", "off"); // keyword + hash vectors, no sidecar needed
        let db = DbHandle::new(std::env::temp_dir().join(format!("allternit-agency-mem-{}.db", uuid::Uuid::new_v4()))).unwrap();
        {
            let conn = db.connect().unwrap();
            for (id, user, fact) in [
                ("f1", "alice", "Alice's quarterly report is due on the first Friday of each quarter."),
                ("f2", "alice", "Alice prefers summaries as bullet points."),
                ("f3", "bob", "Bob's quarterly report goes to the finance channel."),
            ] {
                conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, ?2, ?3, 0.9)", params![id, user, fact])
                .unwrap();
            }
        }
        let r = recall(&db, "alice", "write the quarterly report").await.unwrap();
        assert!(r.ids.contains(&"fact:f1".to_string()), "{:?}", r.ids);
        assert!(!r.ids.iter().any(|i| i.ends_with(":f3")), "another owner's memory leaked: {:?}", r.ids);
        assert!(r.text.contains("first Friday"), "{}", r.text);
        let none = recall(&db, "carol", "write the quarterly report").await.unwrap();
        assert!(none.text.is_empty() && none.ids.is_empty());
    }

    #[tokio::test]
    async fn recall_opens_with_entity_pages_and_follows_their_links() {
        std::env::set_var("ALLTERNIT_EMBED_URL", "off");
        let db = DbHandle::new(std::env::temp_dir().join(format!("allternit-agency-ent-{}.db", uuid::Uuid::new_v4()))).unwrap();
        {
            let conn = db.connect().unwrap();
            for (id, user, fact) in [
                ("f1", "alice", "Project Falcon ships to Northwind Traders in July."),
                ("f2", "alice", "Dana Whitfield owns Project Falcon."),
                ("f3", "bob", "Project Falcon is Bob's codename for the garden shed."),
                ("f4", "bob", "Project Falcon needs more cedar."),
            ] {
                conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, ?2, ?3, 0.9)", params![id, user, fact]).unwrap();
            }
            // The document never says "status" or "update": only the entity link finds it.
            conn.execute(
                "INSERT INTO memory_documents (id, user_id, title, content) VALUES ('d1', 'alice', 'Risk log', 'Vendor delay on Project Falcon: the sensor batch is late by two weeks.')",
                [],
            )
            .unwrap();
            crate::memory_index::chunk_pending_documents(&conn, 10).unwrap();
            while crate::memory_entity_pages::run_pass(&conn, 100).unwrap().targets > 0 {}
        }
        let r = recall(&db, "alice", "Prepare the Project Falcon status update").await.unwrap();
        let page = r.ids.iter().position(|i| i.starts_with("entity_page:")).expect("entity page recalled");
        assert_eq!(page, 0, "the page comes before raw hits: {:?}", r.ids);
        assert!(r.text.starts_with("Profile\n- (entity) Project Falcon"), "{}", r.text);
        assert!(r.ids.contains(&"document:d1".to_string()), "the linked document is followed: {:?}", r.ids);
        assert!(r.text.contains("sensor batch is late"), "{}", r.text);
        assert!(!r.text.contains("cedar") && !r.text.contains("garden shed"), "another owner's page leaked: {}", r.text);
        // Facts shown on the page are not repeated as raw hits.
        assert_eq!(r.text.matches("Dana Whitfield owns Project Falcon").count(), 1, "{}", r.text);
        assert!(r.text.len() <= PROFILE_CHARS + MAX_CHARS + OTHER_CHARS + 64);
    }

    #[tokio::test]
    async fn a_past_turn_survives_next_to_an_entity_page() {
        std::env::set_var("ALLTERNIT_EMBED_URL", "off");
        let db = DbHandle::new(std::env::temp_dir().join(format!("allternit-agency-raw-{}.db", uuid::Uuid::new_v4()))).unwrap();
        let turn = {
            let conn = db.connect().unwrap();
            conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES ('f1', 'alice', 'Project Falcon ships to Northwind Traders in July.', 0.9)", []).unwrap();
            conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES ('f2', 'alice', 'Dana Whitfield owns Project Falcon.', 0.9)", []).unwrap();
            while crate::memory_entity_pages::run_pass(&conn, 100).unwrap().targets > 0 {}
            drop(conn);
            let id = crate::memory_kernel_service::record_observation(
                &db,
                "alice",
                None,
                Some("ses_old"),
                "turn_assistant",
                "For Project Falcon we agreed the launch checklist lives in the zephyr-quartz runbook.",
                Some("gizzi:test"),
            )
            .unwrap();
            db.connect().unwrap().execute("UPDATE memory_observations SET timestamp = '2026-11-20 14:05:00' WHERE id = ?1", params![id]).unwrap();
            for _ in 0..4 {
                memory_index::index_pending(&db, memory_index::global(), 512).await.unwrap();
            }
            id
        };
        let r = recall(&db, "alice", "Where is the Project Falcon launch checklist?").await.unwrap();
        assert!(r.ids.iter().any(|i| i.starts_with("entity_page:")), "the page is there: {:?}", r.ids);
        assert!(r.ids.contains(&format!("observation:{turn}")), "the page must not remove the raw turn: {:?}", r.ids);
        assert!(r.text.contains("[2026-11-20 · assistant] For Project Falcon we agreed the launch checklist lives in the zephyr-quartz runbook."), "{}", r.text);
        let past = r.text.split("Past conversations\n").nth(1).expect("raw block").split("Other memory\n").next().unwrap();
        assert!(!past.contains("Northwind"), "facts stay out of the raw list: {past}");
    }
}
