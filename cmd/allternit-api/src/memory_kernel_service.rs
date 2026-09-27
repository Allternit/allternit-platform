//! Native Memory Kernel V2 Service
//!
//! Provides additive, lightweight SQLite-backed memory operations:
//! observations, fact extraction, entity tracking, semantic/keyword recall, and turn retention.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::db::DbHandle;
use crate::llm_gateway::embeddings::generate_local_embedding;

const EMBEDDING_DIM: usize = 384;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryObservation {
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub timestamp: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub fact: String,
    pub confidence: f64,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub source_observation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntity {
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub entity_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub summary: Option<String>,
    pub last_updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRelationship {
    pub id: String,
    pub user_id: String,
    pub source_entity_id: String,
    pub target_entity_id: String,
    pub relation: String,
    pub confidence: f64,
    pub valid_from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub id: String,
    pub item_type: String, // "fact" | "entity" | "observation"
    pub score: f64,
    pub content: String,
    pub metadata: Value,
    pub timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecordObservationRequest {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetainTurnRequest {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecallQuery {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(thiserror::Error, Debug)]
pub enum MemoryKernelError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

fn f32_vec_to_bytes(vec: &[f32]) -> Vec<u8> {
    vec.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let mut arr = [0u8; 4];
            arr.copy_from_slice(chunk);
            f32::from_le_bytes(arr)
        })
        .collect()
}

/// Store or replace a local embedding for a memory target.
pub fn store_embedding(
    db: &DbHandle,
    user_id: &str,
    target_type: &str,
    target_id: &str,
    text: &str,
) -> Result<String, MemoryKernelError> {
    let id = format!("emb_{}", Uuid::new_v4().simple());
    let embedding = generate_local_embedding(text, EMBEDDING_DIM);
    let bytes = f32_vec_to_bytes(&embedding);
    let conn = db.connect()?;
    conn.execute(
        "INSERT INTO memory_embeddings (id, user_id, target_type, target_id, embedding, model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(user_id, target_type, target_id)
         DO UPDATE SET embedding = excluded.embedding, model = excluded.model, created_at = CURRENT_TIMESTAMP",
        params![id, user_id, target_type, target_id, bytes, "local-hash-384"],
    )?;
    Ok(id)
}

/// Load a single memory target as a RecallResult.
fn load_memory_target(
    conn: &rusqlite::Connection,
    target_type: &str,
    target_id: &str,
) -> Result<Option<RecallResult>, MemoryKernelError> {
    match target_type {
        "fact" => conn
            .query_row(
                "SELECT id, fact, confidence, valid_from, source_observation_id FROM memory_facts WHERE id = ?1",
                params![target_id],
                |row| {
                    Ok(RecallResult {
                        id: row.get::<_, String>(0)?,
                        item_type: "fact".to_string(),
                        score: 0.0,
                        content: row.get::<_, String>(1)?,
                        metadata: serde_json::json!({
                            "confidence": row.get::<_, f64>(2)?,
                            "source_observation_id": row.get::<_, Option<String>>(3)?,
                        }),
                        timestamp: row.get::<_, String>(3)?,
                    })
                },
            )
            .optional()
            .map_err(MemoryKernelError::from),
        "entity" => conn
            .query_row(
                "SELECT id, entity_id, name, type, summary, last_updated FROM memory_entities WHERE id = ?1",
                params![target_id],
                |row| {
                    let name: String = row.get(2)?;
                    let etype: String = row.get(3)?;
                    let summary: Option<String> = row.get(4)?;
                    Ok(RecallResult {
                        id: row.get::<_, String>(0)?,
                        item_type: "entity".to_string(),
                        score: 0.0,
                        content: format!("[Entity: {} ({})] {}", name, etype, summary.as_deref().unwrap_or("")),
                        metadata: serde_json::json!({
                            "entity_id": row.get::<_, String>(1)?,
                            "name": name,
                            "type": etype,
                            "summary": summary,
                        }),
                        timestamp: row.get::<_, String>(5)?,
                    })
                },
            )
            .optional()
            .map_err(MemoryKernelError::from),
        "observation" => conn
            .query_row(
                "SELECT id, kind, content, timestamp, source FROM memory_observations WHERE id = ?1",
                params![target_id],
                |row| {
                    Ok(RecallResult {
                        id: row.get::<_, String>(0)?,
                        item_type: "observation".to_string(),
                        score: 0.0,
                        content: row.get::<_, String>(2)?,
                        metadata: serde_json::json!({
                            "kind": row.get::<_, String>(1)?,
                            "source": row.get::<_, Option<String>>(4)?,
                        }),
                        timestamp: row.get::<_, String>(3)?,
                    })
                },
            )
            .optional()
            .map_err(MemoryKernelError::from),
        _ => Ok(None),
    }
}

/// Find memory targets whose embeddings are most similar to the query text.
pub fn recall_semantic(
    db: &DbHandle,
    user_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<(String, String, f64)>, MemoryKernelError> {
    let conn = db.connect()?;
    let query_vec = generate_local_embedding(query, EMBEDDING_DIM);
    let mut stmt = conn.prepare(
        "SELECT target_type, target_id, embedding FROM memory_embeddings WHERE user_id = ?1",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;

    let mut scored: Vec<(String, String, f64)> = Vec::new();
    for row in rows.flatten() {
        let (target_type, target_id, bytes) = row;
        let candidate = bytes_to_f32_vec(&bytes);
        if candidate.len() == query_vec.len() {
            let sim = cosine_similarity(&query_vec, &candidate) as f64;
            if sim > 0.0 {
                scored.push((target_type, target_id, sim));
            }
        }
    }
    scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
}

/// Record a raw observation (turn, tool execution, file event, decision, or checkpoint).
pub fn record_observation(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    kind: &str,
    content: &str,
    source: Option<&str>,
) -> Result<String, MemoryKernelError> {
    let id = format!("obs_{}", Uuid::new_v4().simple());
    let conn = db.connect()?;

    conn.execute(
        "INSERT INTO memory_observations (id, user_id, agent_id, session_id, kind, content, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, user_id, agent_id, session_id, kind, content, source],
    )?;

    Ok(id)
}

/// Simple heuristic fact extraction (extracts concise sentence declarations).
pub fn extract_facts_heuristic(content: &str) -> Vec<String> {
    let mut facts = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Check for bullet items or key statements
        let clean = trimmed.trim_start_matches(|c| c == '-' || c == '*' || c == '•').trim();
        if clean.len() > 10 && clean.len() < 300 {
            // Heuristic filter: captures declarative sentences or key points
            if clean.contains(" is ")
                || clean.contains(" are ")
                || clean.contains(" preference")
                || clean.contains(" decided")
                || clean.contains(" uses ")
                || clean.contains(" configured")
                || clean.contains(" created")
                || clean.starts_with("User:")
                || clean.starts_with("Goal:")
            {
                facts.push(clean.to_string());
            }
        }
    }
    facts
}

/// Persist extracted facts linked to an observation and index embeddings for semantic recall.
pub fn persist_facts(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    observation_id: &str,
    facts: &[String],
) -> Result<Vec<MemoryFact>, MemoryKernelError> {
    let conn = db.connect()?;
    let mut persisted = Vec::new();

    for fact in facts {
        let fact_id = format!("fact_{}", Uuid::new_v4().simple());
        conn.execute(
            "INSERT INTO memory_facts (id, user_id, agent_id, fact, confidence, source_observation_id)
             VALUES (?1, ?2, ?3, ?4, 0.85, ?5)",
            params![fact_id, user_id, agent_id, fact, observation_id],
        )?;

        // Best-effort embedding index; failures should not block fact persistence.
        let _ = store_embedding(db, user_id, "fact", &fact_id, fact);

        persisted.push(MemoryFact {
            id: fact_id,
            user_id: user_id.to_string(),
            agent_id: agent_id.map(|s| s.to_string()),
            fact: fact.to_string(),
            confidence: 0.85,
            valid_from: chrono::Utc::now().to_rfc3339(),
            valid_until: None,
            source_observation_id: Some(observation_id.to_string()),
        });
    }

    Ok(persisted)
}

/// Retain an agent/user turn: logs an observation and automatically extracts facts.
pub fn retain_turn(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    role: &str,
    content: &str,
) -> Result<String, MemoryKernelError> {
    let kind = format!("turn_{}", role);
    let obs_id = record_observation(db, user_id, agent_id, session_id, &kind, content, Some(role))?;

    let facts = extract_facts_heuristic(content);
    if !facts.is_empty() {
        let _ = persist_facts(db, user_id, agent_id, &obs_id, &facts);
    }

    Ok(obs_id)
}

/// Recall memories matching a query across facts, entities, and recent observations.
pub fn recall(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    session_id: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<RecallResult>, MemoryKernelError> {
    let conn = db.connect()?;
    let words: Vec<String> = query
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .map(|w| format!("%{}%", w.to_lowercase()))
        .collect();

    let mut results: Vec<RecallResult> = Vec::new();

    // 1. Search facts
    let mut fact_stmt = conn.prepare(
        "SELECT id, fact, confidence, valid_from, source_observation_id
         FROM memory_facts
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY valid_from DESC
         LIMIT 50",
    )?;

    let fact_rows = fact_stmt.query_map(params![user_id, agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    for row in fact_rows.flatten() {
        let (id, fact_text, confidence, valid_from, src_obs) = row;
        let lower = fact_text.to_lowercase();
        let match_count = words.iter().filter(|&w| lower.contains(&w[1..w.len() - 1])).count();
        let score = if words.is_empty() {
            confidence
        } else {
            (match_count as f64 / words.len().max(1) as f64) * confidence + 0.1
        };

        if score > 0.1 || words.is_empty() {
            results.push(RecallResult {
                id,
                item_type: "fact".to_string(),
                score,
                content: fact_text,
                metadata: serde_json::json!({
                    "confidence": confidence,
                    "source_observation_id": src_obs,
                }),
                timestamp: valid_from,
            });
        }
    }

    // 2. Search entities
    let mut entity_stmt = conn.prepare(
        "SELECT id, entity_id, name, type, summary, last_updated
         FROM memory_entities
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY last_updated DESC
         LIMIT 30",
    )?;

    let entity_rows = entity_stmt.query_map(params![user_id, agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;

    for row in entity_rows.flatten() {
        let (id, entity_id, name, etype, summary, updated) = row;
        let search_text = format!("{} {} {}", name, etype, summary.as_deref().unwrap_or("")).to_lowercase();
        let match_count = words.iter().filter(|&w| search_text.contains(&w[1..w.len() - 1])).count();
        let score = if words.is_empty() {
            0.5
        } else {
            (match_count as f64 / words.len().max(1) as f64) * 0.9
        };

        if score > 0.1 || words.is_empty() {
            results.push(RecallResult {
                id,
                item_type: "entity".to_string(),
                score,
                content: format!("[Entity: {} ({})] {}", name, etype, summary.as_deref().unwrap_or("")),
                metadata: serde_json::json!({
                    "entity_id": entity_id,
                    "name": name,
                    "type": etype,
                }),
                timestamp: updated,
            });
        }
    }

    // 3. Search observations (fallback/recent context)
    let mut obs_stmt = conn.prepare(
        "SELECT id, kind, content, timestamp, source
         FROM memory_observations
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY timestamp DESC
         LIMIT 20",
    )?;

    let obs_rows = obs_stmt.query_map(params![user_id, agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    for row in obs_rows.flatten() {
        let (id, kind, content, ts, src) = row;
        let lower = content.to_lowercase();
        let match_count = words.iter().filter(|&w| lower.contains(&w[1..w.len() - 1])).count();
        let score = if words.is_empty() {
            0.3
        } else {
            (match_count as f64 / words.len().max(1) as f64) * 0.7
        };

        if score > 0.15 {
            results.push(RecallResult {
                id,
                item_type: "observation".to_string(),
                score,
                content,
                metadata: serde_json::json!({
                    "kind": kind,
                    "source": src,
                }),
                timestamp: ts,
            });
        }
    }

    // 4. Augment with semantic/embedding recall when embeddings exist.
    let semantic_hits = recall_semantic(db, user_id, query, limit.max(20))?;
    if !semantic_hits.is_empty() {
        for (target_type, target_id, sim) in semantic_hits {
            let key = format!("{}:{}", target_type, target_id);
            if let Some(pos) = results.iter().position(|r| format!("{}:{}", r.item_type, r.id) == key) {
                // Boost existing keyword result with semantic signal.
                results[pos].score += sim * 0.35;
                results[pos].metadata["semantic_similarity"] = serde_json::json!(sim);
            } else if let Some(mut hit) = load_memory_target(&conn, &target_type, &target_id)? {
                hit.score = sim * 0.35;
                hit.metadata["semantic_similarity"] = serde_json::json!(sim);
                results.push(hit);
            }
        }
    }

    // 5-Way Reciprocal Rank Fusion (RRF):
    // Blend Lexical (0.25), Confidence/Semantic (0.35), Graph Entity (0.20), and Recency (0.20)
    let k = 60.0;
    let now = chrono::Utc::now();
    for (rank, item) in results.iter_mut().enumerate() {
        let rank_score = 1.0 / (k + (rank as f64) + 1.0);
        let time_score = if let Ok(parsed_ts) = chrono::DateTime::parse_from_rfc3339(&item.timestamp) {
            let age_hours = (now - parsed_ts.with_timezone(&chrono::Utc)).num_hours().max(0) as f64;
            // Half-life decay over 72 hours
            (-age_hours / 72.0).exp()
        } else {
            0.5
        };

        let type_weight = match item.item_type.as_str() {
            "fact" => 1.2,
            "entity" => 1.0,
            _ => 0.8,
        };

        // Reciprocal Rank Fusion formula
        item.score = (item.score * 0.35 + rank_score * 0.25 + time_score * 0.20) * type_weight;
    }

    // Sort by final fused RRF score descending, then by timestamp
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);

    // Record recall log
    let log_id = format!("rec_{}", Uuid::new_v4().simple());
    let results_json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string());
    let _ = conn.execute(
        "INSERT INTO memory_recall_logs (id, user_id, agent_id, session_id, query, results)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![log_id, user_id, agent_id, session_id, query, results_json],
    );

    Ok(results)
}

/// Cosine similarity helper between two float vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// List recent observations.
pub fn list_observations(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryObservation>, MemoryKernelError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, user_id, agent_id, session_id, kind, content, timestamp, source
         FROM memory_observations
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY timestamp DESC
         LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![user_id, agent_id, limit as i64], |row| {
        Ok(MemoryObservation {
            id: row.get(0)?,
            user_id: row.get(1)?,
            agent_id: row.get(2)?,
            session_id: row.get(3)?,
            kind: row.get(4)?,
            content: row.get(5)?,
            timestamp: row.get(6)?,
            source: row.get(7)?,
        })
    })?;

    let mut observations = Vec::new();
    for obs in rows.flatten() {
        observations.push(obs);
    }
    Ok(observations)
}

/// List recent facts.
pub fn list_facts(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryFact>, MemoryKernelError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, user_id, agent_id, fact, confidence, valid_from, valid_until, source_observation_id
         FROM memory_facts
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY valid_from DESC
         LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![user_id, agent_id, limit as i64], |row| {
        Ok(MemoryFact {
            id: row.get(0)?,
            user_id: row.get(1)?,
            agent_id: row.get(2)?,
            fact: row.get(3)?,
            confidence: row.get(4)?,
            valid_from: row.get(5)?,
            valid_until: row.get(6)?,
            source_observation_id: row.get(7)?,
        })
    })?;

    let mut facts = Vec::new();
    for f in rows.flatten() {
        facts.push(f);
    }
    Ok(facts)
}

/// List entities.
pub fn list_entities(
    db: &DbHandle,
    user_id: &str,
    agent_id: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryEntity>, MemoryKernelError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, user_id, agent_id, entity_id, name, type, summary, last_updated
         FROM memory_entities
         WHERE user_id = ?1 AND (agent_id IS NULL OR agent_id = ?2 OR ?2 IS NULL)
         ORDER BY last_updated DESC
         LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![user_id, agent_id, limit as i64], |row| {
        Ok(MemoryEntity {
            id: row.get(0)?,
            user_id: row.get(1)?,
            agent_id: row.get(2)?,
            entity_id: row.get(3)?,
            name: row.get(4)?,
            entity_type: row.get(5)?,
            summary: row.get(6)?,
            last_updated: row.get(7)?,
        })
    })?;

    let mut entities = Vec::new();
    for e in rows.flatten() {
        entities.push(e);
    }
    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v1 = vec![1.0, 2.0, 3.0];
        let v2 = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v1, &v2);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let v1 = vec![1.0, 0.0];
        let v2 = vec![0.0, 1.0];
        let sim = cosine_similarity(&v1, &v2);
        assert!((sim - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_extract_facts_heuristic() {
        let text = "Goal: Build a high-performance bot.\n- The agent uses Rust for its memory kernel.\nRandom line here.\n- User: John Doe is the primary administrator.";
        let facts = extract_facts_heuristic(text);
        assert!(facts.len() >= 2);
        assert!(facts.iter().any(|f| f.contains("Rust")));
        assert!(facts.iter().any(|f| f.contains("John Doe")));
    }

    #[test]
    fn embedding_byte_roundtrip() {
        let vec = vec![1.0f32, -2.5, 3.75, 0.0];
        let bytes = f32_vec_to_bytes(&vec);
        let restored = bytes_to_f32_vec(&bytes);
        assert_eq!(vec, restored);
    }

    #[test]
    fn local_embedding_has_configured_dimensions() {
        let emb = generate_local_embedding("hello world", EMBEDDING_DIM);
        assert_eq!(emb.len(), EMBEDDING_DIM);
    }
}
