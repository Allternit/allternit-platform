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
//! `ALLTERNIT_AGENCY_MEMORY=0` turns it off.

use crate::db::DbHandle;
use crate::memory_index::{self, InputType};
use crate::memory_retrieve::{anchors, Space};

/// Hits handed to the model, and the cap on their text.
const MAX_HITS: usize = 8;
const MAX_CHARS: usize = 4000;
const PER_SPACE: usize = 3;

pub fn enabled() -> bool {
    std::env::var("ALLTERNIT_AGENCY_MEMORY").map(|v| v.trim() != "0").unwrap_or(true)
}

/// The recalled memory for `goal`: a text block (empty when nothing matched)
/// and the ids of the items in it, as `type:id`.
pub struct Recall {
    pub text: String,
    pub ids: Vec<String>,
}

pub async fn recall(db: &DbHandle, owner: &str, goal: &str) -> Result<Recall, String> {
    let qv = memory_index::global().embed_or_hash(&[goal.to_string()], InputType::Query).await;
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut hits = vec![];
    for space in Space::ALL {
        hits.extend(anchors(&conn, *space, owner, None, goal, Some(&qv), PER_SPACE, false).map_err(|e| e.to_string())?);
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut text = String::new();
    let mut ids = vec![];
    for h in hits.into_iter().take(MAX_HITS) {
        let line = format!("- ({}) {}\n", h.item_type, h.content.split_whitespace().collect::<Vec<_>>().join(" "));
        if text.len() + line.len() > MAX_CHARS {
            break;
        }
        text.push_str(&line);
        ids.push(format!("{}:{}", h.item_type, h.id));
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
}
