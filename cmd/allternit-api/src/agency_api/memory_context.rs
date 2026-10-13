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
//! Entity pages first (C3, `memory_entity_pages`): when the goal names
//! entities the owner's memory knows, their compact pages (name, aliases,
//! top facts, related entities) open the block, then the documents that
//! mention them (followed links, a passage around the mention), then the raw
//! hits, minus anything a page already showed. The total stays within the
//! old budget (8 items, 4000 chars): pages take at most [`PAGES_CHARS`] and
//! followed links at most [`MAX_LINKED`] of the 8 item slots, so raw hits
//! keep at least 5 slots and ~1600 chars; with no known entity in the goal
//! the block is exactly the old one.
//!
//! `ALLTERNIT_AGENCY_MEMORY=0` turns it off; `ALLTERNIT_AGENCY_ENTITY_PAGES=0`
//! keeps recall but drops the entity pages.

use std::collections::HashSet;

use crate::db::DbHandle;
use crate::memory_entity_pages;
use crate::memory_index::{self, InputType};
use crate::memory_retrieve::{anchors, Space};

/// Hits handed to the model, and the cap on their text.
const MAX_HITS: usize = 8;
const MAX_CHARS: usize = 4000;
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
    let mut text = String::new();
    let mut ids = vec![];
    // (type, id) already in the block; documents by document id.
    let mut shown: HashSet<(String, String)> = HashSet::new();
    let mut lines = 0;

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
        if text.len() + block.len() > PAGES_CHARS {
            break;
        }
        text.push_str(&block);
        ids.push(format!("entity_page:{}", p.id));
        shown.extend(items);
    }
    let shown_pages = ids.len();
    // Follow the pages' links: the documents that mention the entities.
    let linked = pages.iter().take(shown_pages).flat_map(|p| p.documents.iter()).map(|d| ("document", d)).take(MAX_LINKED * 2);
    for (t, d) in linked {
        if lines >= MAX_LINKED || !shown.insert((t.to_string(), d.id.clone())) {
            continue;
        }
        let Some((title, passage)) = memory_entity_pages::linked_passage(&conn, owner, t, &d.id, d.pos, LINKED_CHARS).map_err(|e| e.to_string())? else {
            continue;
        };
        let line = format!("- ({t}) {}: {}\n", one_line(&title), one_line(&passage));
        if text.len() + line.len() > MAX_CHARS {
            break;
        }
        text.push_str(&line);
        ids.push(format!("{t}:{}", d.id));
        lines += 1;
    }

    let mut hits = vec![];
    for space in Space::ALL {
        hits.extend(anchors(&conn, *space, owner, None, goal, Some(&qv), PER_SPACE, false).map_err(|e| e.to_string())?);
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    for h in hits {
        if lines >= MAX_HITS {
            break;
        }
        let doc = (h.item_type == "document").then(|| h.metadata["source_id"].as_str().map(str::to_string)).flatten();
        if shown.contains(&(h.item_type.clone(), h.id.clone())) || doc.is_some_and(|d| shown.contains(&("document".to_string(), d))) {
            continue;
        }
        let line = format!("- ({}) {}\n", h.item_type, one_line(&h.content));
        if text.len() + line.len() > MAX_CHARS {
            break;
        }
        text.push_str(&line);
        ids.push(format!("{}:{}", h.item_type, h.id));
        lines += 1;
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
        assert!(r.text.starts_with("- (entity) Project Falcon"), "{}", r.text);
        assert!(r.ids.contains(&"document:d1".to_string()), "the linked document is followed: {:?}", r.ids);
        assert!(r.text.contains("sensor batch is late"), "{}", r.text);
        assert!(!r.text.contains("cedar") && !r.text.contains("garden shed"), "another owner's page leaked: {}", r.text);
        // Facts shown on the page are not repeated as raw hits.
        assert_eq!(r.text.matches("Dana Whitfield owns Project Falcon").count(), 1, "{}", r.text);
        assert!(r.ids.len() <= 1 + MAX_HITS && r.text.len() <= MAX_CHARS);
    }
}
