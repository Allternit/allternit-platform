//! Memory-plane index (WP-M1a, decisions O6 + M1).
//!
//! One shared retrieval index for memory facts, entities, observations and
//! document / vector-store chunks:
//! - **Keyword:** SQLite FTS5 (`memory_index_fts`, kept in sync by V206
//!   triggers), ranked by bm25.
//! - **Vector:** real local embeddings from an OpenAI-compatible
//!   `/v1/embeddings` endpoint at `ALLTERNIT_EMBED_URL` (default: the local
//!   sidecar `tools/system-one-local/laya/serve-embed.sh` on :7719). When the
//!   endpoint is down the deterministic hash embedding is the fallback. Each
//!   row records its model id + dimension; vectors are only compared within
//!   one model, and the indexer re-embeds rows made by any other model.
//! - **Fusion:** reciprocal rank fusion (k = 60) of the two ranked lists.
//!
//! Vector search choice: a bounded exact cosine scan in-process (at most
//! [`VECTOR_SCAN_CAP`] most recent rows per (scope, model)). sqlite-vec's
//! `vec0` is itself an exact (brute-force) KNN scan, so it would not change
//! the asymptotics, and it would add a C extension to every Desktop target.
//! Per-user memory is thousands of rows, where an exact scan is a few ms.
//! TODO(M1c): persisted HNSW once a scope exceeds the cap.
//!
//! Backfill: [`index_pending`] picks rows whose embedding is missing or from
//! another model, in batches. It is stateless (progress is the data), so it
//! resumes after any restart. [`spawn_indexer`] runs it in the background.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{params, params_from_iter, Connection};
use serde_json::json;

use crate::db::DbHandle;
use crate::llm_gateway::embeddings::generate_local_embedding;

pub const HASH_MODEL: &str = "local-hash-384";
pub const HASH_DIM: usize = 384;
pub const DEFAULT_EMBED_URL: &str = "http://127.0.0.1:7719";
pub const DEFAULT_EMBED_MODEL: &str = "nomic-ai/modernbert-embed-base";
/// Most recent embedding rows scanned per (scope, model) in vector search.
pub const VECTOR_SCAN_CAP: usize = 20_000;
const RRF_K: f64 = 60.0;
const CHUNK_CHARS: usize = 1200;
const CHUNK_OVERLAP: usize = 200;
/// Long observations are embedded by their head only.
const EMBED_TEXT_CHARS: usize = 4000;
/// Scope used for gateway files attached to vector stores.
pub const FILES_SCOPE: &str = "__files__";

pub const MEMORY_TYPES: &[&str] = &["fact", "entity", "observation"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputType {
    Query,
    Document,
}

impl InputType {
    fn as_str(self) -> &'static str {
        match self {
            InputType::Query => "query",
            InputType::Document => "document",
        }
    }
}

/// Embeddings for a batch of texts, all from one model.
#[derive(Debug, Clone)]
pub struct Embedded {
    pub model: String,
    pub dim: usize,
    pub vectors: Vec<Vec<f32>>,
}

fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Deterministic hash embeddings: the fallback when no endpoint is up.
pub fn hash_embed(texts: &[String]) -> Embedded {
    let vectors = texts
        .iter()
        .map(|t| {
            let mut v = generate_local_embedding(t, HASH_DIM);
            normalize(&mut v);
            v
        })
        .collect();
    Embedded { model: HASH_MODEL.to_string(), dim: HASH_DIM, vectors }
}

/// Client for the local OpenAI-compatible embeddings endpoint.
pub struct EmbedClient {
    url: Option<String>,
    /// Model id last reported by the endpoint (starts as the configured one).
    model: Mutex<String>,
    http: reqwest::Client,
}

impl EmbedClient {
    /// `url: None` disables remote embeddings (hash fallback only).
    pub fn new(url: Option<String>, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            url: url.map(|u| u.trim_end_matches('/').to_string()),
            model: Mutex::new(model.to_string()),
            http,
        }
    }

    /// `ALLTERNIT_EMBED_URL` (unset = local default, `off` = disabled) and
    /// `ALLTERNIT_EMBED_MODEL`.
    pub fn from_env() -> Self {
        let url = match std::env::var("ALLTERNIT_EMBED_URL") {
            Ok(v) if v.trim().is_empty() || v.eq_ignore_ascii_case("off") => None,
            Ok(v) => Some(v),
            Err(_) => Some(DEFAULT_EMBED_URL.to_string()),
        };
        let model = std::env::var("ALLTERNIT_EMBED_MODEL")
            .unwrap_or_else(|_| DEFAULT_EMBED_MODEL.to_string());
        Self::new(url, &model)
    }

    /// The model the index should hold vectors for when the endpoint is up.
    pub fn preferred_model(&self) -> String {
        self.model.lock().map(|m| m.clone()).unwrap_or_default()
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn is_enabled(&self) -> bool {
        self.url.is_some()
    }

    /// Embed through the endpoint. Errors when it is disabled, down or bad.
    pub async fn embed_remote(&self, texts: &[String], kind: InputType) -> Result<Embedded, String> {
        let url = self.url.as_ref().ok_or("embedding endpoint disabled")?;
        if texts.is_empty() {
            return Ok(Embedded { model: self.preferred_model(), dim: 0, vectors: vec![] });
        }
        let body = json!({
            "model": self.preferred_model(),
            "input": texts,
            "input_type": kind.as_str(),
        });
        let resp = self
            .http
            .post(format!("{url}/v1/embeddings"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("embed request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("embed endpoint returned {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| format!("embed response: {e}"))?;
        let mut data: Vec<(usize, Vec<f32>)> = v["data"]
            .as_array()
            .ok_or("embed response has no data")?
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let idx = d["index"].as_u64().map(|x| x as usize).unwrap_or(i);
                let vec = d["embedding"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect())
                    .unwrap_or_default();
                (idx, vec)
            })
            .collect();
        data.sort_by_key(|(i, _)| *i);
        if data.len() != texts.len() {
            return Err(format!("embed endpoint returned {} vectors for {} inputs", data.len(), texts.len()));
        }
        let dim = data[0].1.len();
        if dim == 0 || data.iter().any(|(_, v)| v.len() != dim) {
            return Err("embed endpoint returned empty or ragged vectors".into());
        }
        let model = v["model"].as_str().filter(|m| !m.is_empty()).map(str::to_string)
            .unwrap_or_else(|| self.preferred_model());
        if let Ok(mut m) = self.model.lock() {
            *m = model.clone();
        }
        let vectors = data
            .into_iter()
            .map(|(_, mut v)| {
                normalize(&mut v);
                v
            })
            .collect();
        Ok(Embedded { model, dim, vectors })
    }

    /// Embed through the endpoint, or with the hash fallback when it fails.
    pub async fn embed_or_hash(&self, texts: &[String], kind: InputType) -> Embedded {
        match self.embed_remote(texts, kind).await {
            Ok(e) => e,
            Err(err) => {
                tracing::debug!(error = %err, "memory index: embedding endpoint unavailable, using hash fallback");
                hash_embed(texts)
            }
        }
    }
}

/// Process-wide client configured from the environment.
pub fn global() -> &'static EmbedClient {
    static CLIENT: OnceLock<EmbedClient> = OnceLock::new();
    CLIENT.get_or_init(EmbedClient::from_env)
}

// ─── Storage ────────────────────────────────────────────────────────────────

pub fn f32_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn bytes_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Insert or replace the embedding of one target (one row per target).
pub fn upsert_embedding(
    conn: &Connection,
    scope: &str,
    target_type: &str,
    target_id: &str,
    model: &str,
    vector: &[f32],
) -> rusqlite::Result<()> {
    let id = format!("emb_{}", uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO memory_embeddings (id, user_id, target_type, target_id, embedding, model, dim)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(user_id, target_type, target_id) DO UPDATE SET
           embedding = excluded.embedding, model = excluded.model, dim = excluded.dim,
           created_at = CURRENT_TIMESTAMP",
        params![id, scope, target_type, target_id, f32_to_bytes(vector), model, vector.len() as i64],
    )?;
    Ok(())
}

// ─── Search ─────────────────────────────────────────────────────────────────

/// What a search may return.
pub struct Scope<'a> {
    /// Owner: a user id, or [`FILES_SCOPE`].
    pub scope: &'a str,
    pub target_types: &'a [&'a str],
    /// Restrict to these target ids (e.g. the chunks of one vector store).
    pub only_ids: Option<&'a HashSet<String>>,
}

impl Scope<'_> {
    fn allows(&self, id: &str) -> bool {
        self.only_ids.map_or(true, |ids| ids.contains(id))
    }
    fn type_placeholders(&self, first: usize) -> String {
        (0..self.target_types.len()).map(|i| format!("?{}", first + i)).collect::<Vec<_>>().join(",")
    }
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "was", "what", "who", "how", "why", "when", "where", "which",
    "does", "did", "you", "your", "with", "that", "this", "have", "has", "can", "about", "from",
    "into", "any", "all", "its", "our", "out", "but", "not", "his", "her", "they", "them",
];

/// FTS5 MATCH expression for a free-text query: quoted tokens OR-ed together
/// (bm25 ranks documents that match more of them higher). None when nothing
/// searchable is left.
pub fn fts_query(text: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let toks: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.chars().count() >= 2 && !STOPWORDS.contains(&t.as_str()))
        .filter(|t| seen.insert(t.clone()))
        .map(|t| format!("\"{t}\""))
        .collect();
    (!toks.is_empty()).then(|| toks.join(" OR "))
}

pub type TargetKey = (String, String); // (target_type, target_id)

/// Keyword candidates ranked by bm25 (best first).
pub fn keyword_search(conn: &Connection, scope: &Scope, query: &str, limit: usize) -> rusqlite::Result<Vec<TargetKey>> {
    let Some(m) = fts_query(query) else { return Ok(vec![]) };
    let sql = format!(
        "SELECT target_type, target_id FROM memory_index_fts
         WHERE memory_index_fts MATCH ?1 AND user_id = ?2 AND target_type IN ({})
         ORDER BY bm25(memory_index_fts) LIMIT {}",
        scope.type_placeholders(3),
        // Over-fetch when filtering by id afterwards.
        if scope.only_ids.is_some() { limit * 20 } else { limit }
    );
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&m, &scope.scope];
    for t in scope.target_types {
        args.push(t);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for r in rows {
        let (t, id) = r?;
        if scope.allows(&id) {
            out.push((t, id));
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

/// Vector candidates: exact cosine over the scope's rows from the query's
/// model (bounded by [`VECTOR_SCAN_CAP`]). Returns (key, similarity), best first.
pub fn vector_search(
    conn: &Connection,
    scope: &Scope,
    model: &str,
    query: &[f32],
    limit: usize,
) -> rusqlite::Result<Vec<(TargetKey, f64)>> {
    let sql = format!(
        "SELECT target_type, target_id, embedding FROM memory_embeddings
         WHERE user_id = ?1 AND model = ?2 AND dim = ?3 AND target_type IN ({})
         ORDER BY created_at DESC LIMIT {VECTOR_SCAN_CAP}",
        scope.type_placeholders(4)
    );
    let dim = query.len() as i64;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&scope.scope, &model, &dim];
    for t in scope.target_types {
        args.push(t);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
    })?;
    let mut scored = Vec::new();
    for r in rows {
        let (t, id, bytes) = r?;
        if !scope.allows(&id) {
            continue;
        }
        let v = bytes_to_f32(&bytes);
        let sim = crate::memory_kernel_service::cosine_similarity(query, &v) as f64;
        if sim > 0.0 {
            scored.push(((t, id), sim));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub target_type: String,
    pub target_id: String,
    /// Reciprocal-rank-fusion score.
    pub score: f64,
    pub keyword_rank: Option<usize>,
    pub vector_rank: Option<usize>,
    pub similarity: Option<f64>,
}

/// Hybrid search: FTS5 keyword + vector candidates fused with reciprocal
/// rank fusion. `query_embedding` is the query vector and its model; with
/// None the search is keyword-only.
pub fn hybrid_search(
    conn: &Connection,
    scope: &Scope,
    query: &str,
    query_embedding: Option<(&str, &[f32])>,
    limit: usize,
) -> rusqlite::Result<Vec<Hit>> {
    let pool = limit.max(10) * 3;
    let keyword = keyword_search(conn, scope, query, pool)?;
    let vector = match query_embedding {
        Some((model, v)) if !v.is_empty() => vector_search(conn, scope, model, v, pool)?,
        _ => vec![],
    };
    let mut hits: HashMap<TargetKey, Hit> = HashMap::new();
    fn entry<'a>(hits: &'a mut HashMap<TargetKey, Hit>, key: &TargetKey) -> &'a mut Hit {
        hits.entry(key.clone()).or_insert_with(|| Hit {
            target_type: key.0.clone(),
            target_id: key.1.clone(),
            score: 0.0,
            keyword_rank: None,
            vector_rank: None,
            similarity: None,
        })
    }
    for (rank, key) in keyword.iter().enumerate() {
        let h = entry(&mut hits, key);
        h.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        h.keyword_rank = Some(rank + 1);
    }
    for (rank, (key, sim)) in vector.iter().enumerate() {
        let h = entry(&mut hits, key);
        h.score += 1.0 / (RRF_K + rank as f64 + 1.0);
        h.vector_rank = Some(rank + 1);
        h.similarity = Some(*sim);
    }
    let mut out: Vec<Hit> = hits.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.similarity.unwrap_or(0.0).partial_cmp(&a.similarity.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
    });
    out.truncate(limit);
    Ok(out)
}

// ─── Chunking ───────────────────────────────────────────────────────────────

/// Split text into overlapping chunks of about [`CHUNK_CHARS`] characters,
/// preferring paragraph / sentence / word boundaries.
pub fn chunk_text(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + CHUNK_CHARS).min(chars.len());
        if end < chars.len() {
            let window: String = chars[start..end].iter().collect();
            let cut = ["\n\n", ". ", "\n", " "]
                .iter()
                .find_map(|sep| window.rfind(sep).filter(|&p| p > CHUNK_CHARS / 2).map(|p| p + sep.len()));
            if let Some(byte_cut) = cut {
                end = start + window[..byte_cut].chars().count();
            }
        }
        let piece: String = chars[start..end].iter().collect::<String>().trim().to_string();
        if !piece.is_empty() {
            out.push(piece);
        }
        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
    }
    out
}

fn insert_chunks(conn: &Connection, scope: &str, source_type: &str, source_id: &str, text: &str) -> rusqlite::Result<usize> {
    let chunks = chunk_text(text);
    for (i, c) in chunks.iter().enumerate() {
        conn.execute(
            "INSERT INTO memory_index_chunks (id, scope, source_type, source_id, chunk_index, text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![format!("chk_{}", uuid::Uuid::new_v4().simple()), scope, source_type, source_id, i as i64, c],
        )?;
    }
    conn.execute(
        "INSERT OR REPLACE INTO memory_index_sources (source_type, source_id, chunk_count) VALUES (?1, ?2, ?3)",
        params![source_type, source_id, chunks.len() as i64],
    )?;
    Ok(chunks.len())
}

/// Chunk memory documents not yet in the index. Returns chunks written.
pub fn chunk_pending_documents(conn: &Connection, batch: usize) -> rusqlite::Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.user_id, d.title, COALESCE(d.content, '') FROM memory_documents d
         WHERE NOT EXISTS (SELECT 1 FROM memory_index_sources s
                           WHERE s.source_type = 'document' AND s.source_id = d.id)
         ORDER BY d.created_at LIMIT ?1",
    )?;
    let docs: Vec<(String, String, String, String)> = stmt
        .query_map(params![batch as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let mut written = 0;
    for (id, user, title, content) in docs {
        let n = insert_chunks(conn, &user, "document", &id, &format!("{title}\n\n{content}"))?;
        conn.execute(
            "UPDATE memory_documents SET is_indexed = 1, chunk_count = ?2 WHERE id = ?1",
            params![id, n as i64],
        )?;
        written += n;
    }
    Ok(written)
}

/// Text of a gateway file, when it looks like text.
fn file_text(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(bytes).ok()?;
    (!s.contains('\0')).then(|| s.to_string())
}

/// Chunk gateway files attached to vector stores (all of them, or only
/// `file_ids`) that are not yet in the index. Returns chunks written.
pub fn chunk_pending_files(conn: &Connection, file_ids: Option<&[String]>, batch: usize) -> rusqlite::Result<usize> {
    let has_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'vector_store_files')",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(0);
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT f.id, f.bytes FROM vector_store_files v JOIN files f ON f.id = v.file_id
         WHERE NOT EXISTS (SELECT 1 FROM memory_index_sources s
                           WHERE s.source_type = 'file' AND s.source_id = f.id)
         LIMIT ?1",
    )?;
    let files: Vec<(String, Vec<u8>)> = stmt
        .query_map(params![batch as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut written = 0;
    for (id, bytes) in files {
        if let Some(ids) = file_ids {
            if !ids.contains(&id) {
                continue;
            }
        }
        let text = file_text(&bytes).unwrap_or_default();
        written += insert_chunks(conn, FILES_SCOPE, "file", &id, &text)?;
    }
    Ok(written)
}

// ─── Backfill / re-embed ────────────────────────────────────────────────────

/// A row that needs (re-)embedding.
#[derive(Debug, Clone)]
pub struct PendingTarget {
    pub scope: String,
    pub target_type: String,
    pub target_id: String,
    pub text: String,
}

/// Rows whose embedding is missing, or (when `model` is Some) from another
/// model. Facts first, then entities, chunks, observations.
pub fn pending_targets(conn: &Connection, model: Option<&str>, batch: usize) -> rusqlite::Result<Vec<PendingTarget>> {
    let stale = if model.is_some() { "(e.id IS NULL OR e.model IS NOT ?1)" } else { "(e.id IS NULL AND ?1 IS NULL)" };
    let sources = [
        ("fact", "SELECT f.user_id, f.id, f.fact FROM memory_facts f", "f.user_id", "f.id", "f.valid_until IS NULL"),
        ("entity", "SELECT n.user_id, n.id, n.name || ' ' || n.type || ' ' || COALESCE(n.summary, '') FROM memory_entities n", "n.user_id", "n.id", "1"),
        ("chunk", "SELECT c.scope, c.id, c.text FROM memory_index_chunks c", "c.scope", "c.id", "1"),
        ("observation", "SELECT o.user_id, o.id, o.content FROM memory_observations o", "o.user_id", "o.id", "1"),
    ];
    let mut out = Vec::new();
    for (ttype, select, scope_col, id_col, filter) in sources {
        if out.len() >= batch {
            break;
        }
        let sql = format!(
            "{select} LEFT JOIN memory_embeddings e
               ON e.user_id = {scope_col} AND e.target_type = '{ttype}' AND e.target_id = {id_col}
             WHERE {filter} AND {stale} ORDER BY {id_col} LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![model, (batch - out.len()) as i64], |r| {
            Ok(PendingTarget {
                scope: r.get(0)?,
                target_type: ttype.to_string(),
                target_id: r.get(1)?,
                text: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })?;
        for r in rows {
            let mut p = r?;
            if p.text.chars().count() > EMBED_TEXT_CHARS {
                p.text = p.text.chars().take(EMBED_TEXT_CHARS).collect();
            }
            out.push(p);
        }
    }
    Ok(out)
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct IndexReport {
    pub chunked: usize,
    pub embedded: usize,
    pub model: String,
    pub remote: bool,
}

/// One indexing pass: chunk new documents/files, then embed up to `batch`
/// rows that are missing an embedding or carry one from another model.
/// When the endpoint is down, only rows with no embedding at all get the
/// hash fallback (so keyword+hash search still covers them); they are
/// re-embedded with the real model once it is back.
pub async fn index_pending(db: &DbHandle, client: &EmbedClient, batch: usize) -> Result<IndexReport, String> {
    let db2 = db.clone();
    let chunked = tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let conn = db2.connect()?;
        Ok(chunk_pending_documents(&conn, 16)? + chunk_pending_files(&conn, None, 16)?)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let mut report = IndexReport { chunked, ..Default::default() };
    if client.is_enabled() {
        let model = client.preferred_model();
        let db2 = db.clone();
        let m2 = model.clone();
        let pending = tokio::task::spawn_blocking(move || db2.connect().and_then(|c| pending_targets(&c, Some(&m2), batch)))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if pending.is_empty() {
            report.model = model;
            report.remote = true;
            return Ok(report);
        }
        let texts: Vec<String> = pending.iter().map(|p| p.text.clone()).collect();
        if let Ok(emb) = client.embed_remote(&texts, InputType::Document).await {
            report.model = emb.model.clone();
            report.remote = true;
            report.embedded = write_embeddings(db, pending, emb).await?;
            return Ok(report);
        }
    }
    // Endpoint disabled or down: hash-embed rows that have nothing yet.
    let db2 = db.clone();
    let pending = tokio::task::spawn_blocking(move || db2.connect().and_then(|c| pending_targets(&c, None, batch)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let texts: Vec<String> = pending.iter().map(|p| p.text.clone()).collect();
    let emb = hash_embed(&texts);
    report.model = emb.model.clone();
    report.embedded = write_embeddings(db, pending, emb).await?;
    Ok(report)
}

async fn write_embeddings(db: &DbHandle, pending: Vec<PendingTarget>, emb: Embedded) -> Result<usize, String> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        for (p, v) in pending.iter().zip(emb.vectors.iter()) {
            upsert_embedding(&tx, &p.scope, &p.target_type, &p.target_id, &emb.model, v)?;
        }
        tx.commit()?;
        Ok(pending.len())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Background indexer: drains the backlog in batches, then polls. Backs off
/// while the embedding endpoint is down. `ALLTERNIT_MEMORY_INDEXER=0` disables.
pub fn spawn_indexer(db: DbHandle) {
    if std::env::var("ALLTERNIT_MEMORY_INDEXER").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    tokio::spawn(async move {
        let client = global();
        loop {
            let mut idle = Duration::from_secs(20);
            loop {
                match index_pending(&db, client, 64).await {
                    Ok(r) if r.embedded > 0 || r.chunked > 0 => {
                        tracing::debug!(embedded = r.embedded, chunked = r.chunked, model = %r.model, "memory index pass");
                        if !r.remote && client.is_enabled() {
                            idle = Duration::from_secs(300);
                            break;
                        }
                    }
                    Ok(r) => {
                        if !r.remote && client.is_enabled() {
                            idle = Duration::from_secs(300);
                        }
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "memory index pass failed");
                        idle = Duration::from_secs(300);
                        break;
                    }
                }
            }
            tokio::time::sleep(idle).await;
        }
    });
}

/// Hybrid search over the chunks of the given gateway files (vector-store
/// search). Returns (file_id, chunk text, score), best first.
pub fn search_files(
    conn: &Connection,
    file_ids: &[String],
    query: &str,
    query_embedding: Option<(&str, &[f32])>,
    limit: usize,
) -> rusqlite::Result<Vec<(String, String, f64)>> {
    if file_ids.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = (1..=file_ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT id, source_id, text FROM memory_index_chunks
         WHERE source_type = 'file' AND source_id IN ({placeholders})"
    ))?;
    let mut chunk_file: HashMap<String, (String, String)> = HashMap::new();
    for r in stmt.query_map(params_from_iter(file_ids.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })? {
        let (id, file, text) = r?;
        chunk_file.insert(id, (file, text));
    }
    let ids: HashSet<String> = chunk_file.keys().cloned().collect();
    let scope = Scope { scope: FILES_SCOPE, target_types: &["chunk"], only_ids: Some(&ids) };
    Ok(hybrid_search(conn, &scope, query, query_embedding, limit)?
        .into_iter()
        .filter_map(|h| chunk_file.get(&h.target_id).map(|(f, t)| (f.clone(), t.clone(), h.score)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_kernel_service as kernel;

    /// Concept groups the mock embedding model "understands".
    const CONCEPTS: &[&[&str]] = &[
        &["car", "tesla", "vehicle", "drive", "driving", "drives", "model"],
        &["vegan", "diet", "food", "eat", "meat", "meals"],
        &["engineer", "job", "work", "company", "employer"],
        &["austin", "city", "live", "lives", "texas"],
        &["refund", "policy", "return", "money", "days"],
    ];

    fn mock_vec(text: &str) -> Vec<f32> {
        let mut v = vec![0.01f32; CONCEPTS.len()];
        for w in text.split(|c: char| !c.is_alphanumeric()).map(|w| w.to_lowercase()) {
            for (i, g) in CONCEPTS.iter().enumerate() {
                if g.contains(&w.as_str()) {
                    v[i] += 1.0;
                }
            }
        }
        v
    }

    /// OpenAI-compatible mock /v1/embeddings that reports `model`.
    async fn spawn_mock(model: &'static str) -> String {
        use axum::{routing::post, Json, Router};
        let app = Router::new().route(
            "/v1/embeddings",
            post(move |Json(body): Json<serde_json::Value>| async move {
                let inputs: Vec<String> = match &body["input"] {
                    serde_json::Value::String(s) => vec![s.clone()],
                    v => v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect(),
                };
                let data: Vec<_> = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| json!({"object": "embedding", "index": i, "embedding": mock_vec(t)}))
                    .collect();
                Json(json!({"object": "list", "model": model, "data": data}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    fn down_client() -> EmbedClient {
        // Port 9 (discard) is never served locally: connection refused.
        EmbedClient::new(Some("http://127.0.0.1:9".into()), DEFAULT_EMBED_MODEL)
    }

    fn seed(db: &DbHandle) -> Vec<String> {
        let obs = kernel::record_observation(db, "u1", None, None, "explicit_memory", "x", Some("user")).unwrap();
        let facts = [
            "Owns a Tesla Model 3.",
            "Follows a vegan diet.",
            "Works as an engineer at Acme.",
            "Lives in Austin.",
        ];
        kernel::persist_facts(db, "u1", None, &obs, &facts.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .unwrap()
            .into_iter()
            .map(|f| f.id)
            .collect()
    }

    /// The recall this replaces: substring LIKE match over facts, ranked by
    /// how many query words (>2 chars) occur.
    fn like_baseline(db: &DbHandle, query: &str) -> Vec<(String, usize)> {
        let conn = db.connect().unwrap();
        let words: Vec<String> = query.split_whitespace().filter(|w| w.len() > 2).map(|w| w.to_lowercase()).collect();
        let mut stmt = conn.prepare("SELECT id, fact FROM memory_facts WHERE user_id = 'u1'").unwrap();
        let mut out: Vec<(String, usize)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .map(|(id, f)| {
                let lf = f.to_lowercase();
                (id, words.iter().filter(|w| lf.contains(w.as_str())).count())
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    #[tokio::test]
    async fn hybrid_recall_beats_like_on_fixture() {
        let db = DbHandle::new_memory().unwrap();
        let ids = seed(&db);
        let client = EmbedClient::new(Some(spawn_mock("mock-embed-a").await), "mock-embed-a");
        let r = index_pending(&db, &client, 64).await.unwrap();
        assert!(r.remote && r.embedded >= 4, "{r:?}");

        // Paraphrase: no shared words with the fact, only meaning.
        let q = "which vehicle is mine";
        assert_eq!(like_baseline(&db, q)[0].1, 0, "LIKE finds nothing relevant");
        let hits = kernel::recall_hybrid(&db, &client, "u1", None, None, q, 3).await.unwrap();
        assert_eq!(hits[0].id, ids[0], "hybrid finds the car fact: {hits:?}");
        assert_eq!(hits[0].metadata["embedding_model"], "mock-embed-a");

        // Inflection: "driving" vs "drives" (FTS5 porter stems both).
        let q = "driving";
        let hits = kernel::recall_hybrid(&db, &client, "u1", None, None, "who drives?", 3).await.unwrap();
        assert_eq!(hits[0].id, ids[0]);
        assert_eq!(like_baseline(&db, q)[0].1, 0);

        // Keyword-exact still wins for literal names.
        let hits = kernel::recall_hybrid(&db, &client, "u1", None, None, "Acme", 3).await.unwrap();
        assert_eq!(hits[0].id, ids[2]);
        // Other users never see these.
        assert!(kernel::recall_hybrid(&db, &client, "u2", None, None, "Acme", 3).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn falls_back_to_hash_and_keywords_when_endpoint_is_down() {
        let db = DbHandle::new_memory().unwrap();
        let ids = seed(&db);
        let client = down_client();
        let r = index_pending(&db, &client, 64).await.unwrap();
        assert!(!r.remote);
        assert_eq!(r.model, HASH_MODEL);
        let hits = kernel::recall_hybrid(&db, &client, "u1", None, None, "where does she live, Austin?", 3).await.unwrap();
        assert_eq!(hits[0].id, ids[3]);
        assert_eq!(hits[0].metadata["embedding_model"], HASH_MODEL);
        // Sync recall (hash) agrees.
        assert_eq!(kernel::recall(&db, "u1", None, None, "Austin", 3).unwrap()[0].id, ids[3]);
    }

    #[tokio::test]
    async fn model_change_reembeds_and_backfill_resumes() {
        let db = DbHandle::new_memory().unwrap();
        seed(&db);
        let count = |model: &str| -> i64 {
            db.connect().unwrap()
                .query_row("SELECT COUNT(*) FROM memory_embeddings WHERE model = ?1", [model], |r| r.get(0))
                .unwrap()
        };
        // persist_facts wrote hash vectors at once.
        assert_eq!(count(HASH_MODEL), 4);

        let a = EmbedClient::new(Some(spawn_mock("mock-embed-a").await), "mock-embed-a");
        // Small batches: the backlog drains across passes (resumable).
        let mut passes = 0;
        loop {
            let r = index_pending(&db, &a, 2).await.unwrap();
            passes += 1;
            if r.embedded == 0 {
                break;
            }
        }
        assert!(passes >= 3);
        assert_eq!(count(HASH_MODEL), 0, "hash vectors upgraded");
        let total_a = count("mock-embed-a");
        assert!(total_a >= 5); // 4 facts + 1 observation

        // Model change: every row is re-embedded, never mixed.
        let b = EmbedClient::new(Some(spawn_mock("mock-embed-b").await), "mock-embed-b");
        let r = index_pending(&db, &b, 2).await.unwrap();
        assert_eq!(r.model, "mock-embed-b");
        while index_pending(&db, &b, 64).await.unwrap().embedded > 0 {}
        assert_eq!(count("mock-embed-a"), 0);
        assert_eq!(count("mock-embed-b"), total_a);
        let dims: i64 = db.connect().unwrap()
            .query_row("SELECT MIN(dim) FROM memory_embeddings", [], |r| r.get(0)).unwrap();
        assert_eq!(dims as usize, CONCEPTS.len());
        // Nothing left: a pass is a no-op.
        assert_eq!(index_pending(&db, &b, 64).await.unwrap().embedded, 0);
    }

    #[tokio::test]
    async fn documents_and_vector_store_files_share_the_index() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO memory_documents (id, user_id, title, content) VALUES ('d1', 'u1', 'Handbook', ?1)",
            [format!("{}\n\nRefunds are issued within 14 days of a return.", "Filler text. ".repeat(200))],
        ).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vector_stores (id TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'completed', metadata_json TEXT, expires_at INTEGER, created_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS vector_store_files (id TEXT PRIMARY KEY, vector_store_id TEXT NOT NULL, file_id TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'completed', created_at INTEGER NOT NULL);
             INSERT INTO files (id, filename, bytes, size) VALUES ('file-1', 'policy.txt', CAST('Our money-back policy: returns accepted for 30 days.' AS BLOB), 50);
             INSERT INTO files (id, filename, bytes, size) VALUES ('file-2', 'other.txt', CAST('Unrelated notes about the office plants.' AS BLOB), 40);
             INSERT INTO vector_stores (id, name, created_at) VALUES ('vs1', 's', 0);
             INSERT INTO vector_store_files (id, vector_store_id, file_id, created_at) VALUES ('a', 'vs1', 'file-1', 0), ('b', 'vs1', 'file-2', 0);",
        ).unwrap();
        let client = EmbedClient::new(Some(spawn_mock("mock-embed-a").await), "mock-embed-a");
        let idle = IndexReport { remote: true, model: "mock-embed-a".into(), ..Default::default() };
        while index_pending(&db, &client, 64).await.unwrap() != idle {}

        let chunks: i64 = conn.query_row("SELECT chunk_count FROM memory_documents WHERE id = 'd1'", [], |r| r.get(0)).unwrap();
        assert!(chunks >= 2, "long document is chunked: {chunks}");
        let q = client.embed_or_hash(&["refund policy".into()], InputType::Query).await;
        let scope = Scope { scope: "u1", target_types: &["chunk"], only_ids: None };
        let hits = hybrid_search(&conn, &scope, "refund policy", Some((&q.model, &q.vectors[0])), 3).unwrap();
        let top: String = conn.query_row("SELECT text FROM memory_index_chunks WHERE id = ?1", [&hits[0].target_id], |r| r.get(0)).unwrap();
        assert!(top.contains("Refunds"), "{top}");

        let res = search_files(&conn, &["file-1".into(), "file-2".into()], "refund", Some((&q.model, &q.vectors[0])), 5).unwrap();
        assert_eq!(res[0].0, "file-1");
        // Restricting to other files excludes it.
        let res = search_files(&conn, &["file-2".into()], "refund", None, 5).unwrap();
        assert!(res.iter().all(|r| r.0 == "file-2"));

        // Deleting the document drops its chunks from the index.
        conn.execute("DELETE FROM memory_documents WHERE id = 'd1'", []).unwrap();
        assert!(hybrid_search(&conn, &scope, "refund policy", None, 3).unwrap().is_empty());
    }

    /// Against the real sidecar: `serve-embed.sh` running on ALLTERNIT_EMBED_URL
    /// (default :7719). `cargo test -p allternit-api --lib live_sidecar -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn live_sidecar_recall() {
        let db = DbHandle::new_memory().unwrap();
        let ids = seed(&db);
        let client = EmbedClient::from_env();
        let r = index_pending(&db, &client, 64).await.unwrap();
        assert!(r.remote, "sidecar not reachable: {r:?}");
        let hits = kernel::recall_hybrid(&db, &client, "u1", None, None, "which vehicle is mine", 3).await.unwrap();
        assert_eq!(hits[0].id, ids[0], "{hits:?}");
        assert_eq!(hits[0].metadata["embedding_model"], DEFAULT_EMBED_MODEL);
    }

    #[test]
    fn chunker_overlaps_and_covers_text() {
        let text = "word ".repeat(1000);
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 4);
        assert!(chunks.iter().all(|c| c.chars().count() <= CHUNK_CHARS));
        assert!(chunk_text("").is_empty());
        assert_eq!(fts_query("What is the: \"car\"?").as_deref(), Some("\"is\" OR \"car\""));
    }
}
