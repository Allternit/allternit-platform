//! Raw past-chat search (A4, RAW-RAG arXiv 2610.30755): dense + keyword
//! candidates over headed observation windows → cross-encoder rerank on the
//! local sidecar → top-k windows → expand to the full source turns → budget.
//!
//! The paper's ablations are the design: dropping the reranker cost 3.9
//! points and mixing atomic facts into the pool cost 19.6, so this searches
//! only raw `turn_*` windows (`memory_index_chunks.source_type =
//! 'observation'`, written by `memory_index::chunk_pending_observations`) and
//! returns knowledge files in their own `knowledge` block (step 17), never
//! mixed into `turns`.
//!
//! **Knowledge (step 17):** approved `knowledge/domain/*.md` and
//! `knowledge/data-access/*.md` are Memory Drive topic files. The drive's
//! `index_snapshot` already projects every bullet into `memory_facts` (+
//! embedding + FTS) and records its file in `memory_drive_entries`, and a
//! drive write re-projects them (a removed bullet is retired), so no second
//! index is needed: [`knowledge_hits`] runs its own hybrid search over just
//! those fact ids (personal drive of the user, `valid_until IS NULL`).
//!
//! - **Candidates:** [`memory_index::hybrid_search`] (FTS5 + vector, RRF) over
//!   `chunk` targets, restricted through `Scope::only_ids` to windows whose
//!   observation still exists (inner join on `memory_observations`), belongs
//!   to the user, passes `from`/`to`/`session_id`/`exclude_sessions`, and is
//!   not in an incognito session (`ephemeral_sessions`, the same predicate
//!   `memory_dream::evidence` uses).
//! - **Query vector:** the real embedding from [`EmbedClient`] with the hash
//!   fallback, as `memory_kernel_service::recall_hybrid` does.
//! - **Rerank:** `POST {ALLTERNIT_EMBED_URL}/v1/rerank` on the embed client's
//!   own HTTP client, with a per-mode timeout. On timeout, error, 503 or a
//!   disabled sidecar the RRF order stands and `reranked` is false.
//! - **Budgets per [`RawMode`]** (CONTRACT.md "Raw search"): `auto` = 48
//!   candidates / 600 ms / top 8 / 1,200 tokens; `search` = 128 / 1,500 ms /
//!   top 12 / 1,500 tokens (≈ 4 chars per token). Each turn is clipped at
//!   [`TURN_CLIP_CHARS`] with [`CLIP_MARK`].
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::db::DbHandle;
use crate::memory_index::{self, EmbedClient, InputType, Scope};

/// Each expanded turn is clipped at this many chars.
pub const TURN_CLIP_CHARS: usize = 1200;
/// Appended to a clipped turn.
pub const CLIP_MARK: &str = " …[clipped]";
pub const CHARS_PER_TOKEN: usize = 4;
/// Upper bound on a caller's `limit`.
pub const MAX_LIMIT: usize = 32;
/// Upper bound on `before` / `after` for neighbours.
pub const MAX_NEIGHBORS: usize = 20;

/// Incognito sessions never come back (same predicate as `memory_dream::evidence`).
const NOT_EPHEMERAL: &str =
    "(o.session_id IS NULL OR NOT EXISTS(SELECT 1 FROM ephemeral_sessions s WHERE s.session_id = o.session_id))";
const TURN_COLS: &str = "o.id, o.session_id, o.kind, strftime('%Y-%m-%dT%H:%M:%SZ', o.timestamp), o.source, o.content";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RawMode {
    /// The per-turn "From past chats" block: small and fast.
    Auto,
    /// Tools, `thread_search`, agency: the default.
    #[default]
    Search,
}

/// Limits for one [`RawMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub candidates: usize,
    pub rerank_timeout: Duration,
    pub top: usize,
    pub tokens: usize,
}

impl RawMode {
    pub fn budget(self) -> Budget {
        match self {
            RawMode::Auto => Budget { candidates: 48, rerank_timeout: Duration::from_millis(600), top: 8, tokens: 1200 },
            RawMode::Search => Budget { candidates: 128, rerank_timeout: Duration::from_millis(1500), top: 12, tokens: 1500 },
        }
    }
}

fn null_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(d)?.unwrap_or_default())
}

/// `POST /api/v1/memory/v2/raw-search` body (CONTRACT.md "Raw search").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RawQuery {
    pub query: String,
    /// Inclusive lower bound on `memory_observations.timestamp`: `YYYY-MM-DD` or RFC 3339.
    pub from: Option<String>,
    /// Upper bound: a `YYYY-MM-DD` date includes that whole day; RFC 3339 is inclusive.
    pub to: Option<String>,
    /// Only this session.
    pub session_id: Option<String>,
    #[serde(deserialize_with = "null_vec")]
    pub exclude_sessions: Vec<String>,
    /// Windows kept after rerank (default: the mode's top, max [`MAX_LIMIT`]).
    pub limit: Option<usize>,
    /// Fill `knowledge` with the top approved knowledge files (separate from `turns`).
    pub include_knowledge: bool,
    pub mode: RawMode,
}

/// One past turn, expanded from its best windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawTurn {
    pub observation_id: String,
    pub session_id: Option<String>,
    /// `user`, `assistant` or `tool:<name>`.
    pub speaker: String,
    /// RFC 3339 UTC, e.g. `2026-11-20T14:05:00Z`.
    pub timestamp: String,
    pub source: Option<String>,
    /// The full turn, clipped at [`TURN_CLIP_CHARS`] (marked with [`CLIP_MARK`]).
    pub text: String,
    /// Rerank relevance when `reranked`, else the RRF score. 0 for neighbours.
    pub score: f64,
    /// The selected windows of this turn, best first.
    pub window_ids: Vec<String>,
}

/// A knowledge file hit (step 17). Never mixed into `turns`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHit {
    pub path: String,
    /// `domain` or `data_access`.
    pub kind: String,
    pub text: String,
    pub score: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawResult {
    pub turns: Vec<RawTurn>,
    pub knowledge: Vec<KnowledgeHit>,
    /// False when the sidecar's rerank was unavailable and RRF order stands.
    pub reranked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawError {
    /// The request is malformed (e.g. a bad `from`/`to`): HTTP 400.
    BadRequest(String),
    Internal(String),
}

impl std::fmt::Display for RawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RawError::BadRequest(m) => write!(f, "bad request: {m}"),
            RawError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for RawError {}

fn internal<E: std::fmt::Display>(e: E) -> RawError {
    RawError::Internal(e.to_string())
}

/// A time bound as an SQLite `datetime()` string plus its comparison.
#[derive(Debug, Clone)]
struct Bound {
    at: String,
    op: &'static str,
}

fn parse_bound(field: &str, value: &str, upper: bool) -> Result<Bound, RawError> {
    let v = value.trim();
    if let Ok(d) = chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        if upper {
            // A date-only upper bound includes that whole day.
            let next = d.succ_opt().ok_or_else(|| RawError::BadRequest(format!("`{field}` is out of range")))?;
            return Ok(Bound { at: format!("{} 00:00:00", next.format("%Y-%m-%d")), op: "<" });
        }
        return Ok(Bound { at: format!("{} 00:00:00", d.format("%Y-%m-%d")), op: ">=" });
    }
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(v) {
        let at = t.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M:%S").to_string();
        return Ok(Bound { at, op: if upper { "<=" } else { ">=" } });
    }
    Err(RawError::BadRequest(format!("`{field}` must be YYYY-MM-DD or RFC 3339, got {value:?}")))
}

#[derive(Debug, Clone, Default)]
struct Filters {
    from: Option<Bound>,
    to: Option<Bound>,
    session: Option<String>,
    exclude: Vec<String>,
}

impl Filters {
    fn of(q: &RawQuery) -> Result<Self, RawError> {
        let nonempty = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        Ok(Self {
            from: nonempty(&q.from).map(|v| parse_bound("from", &v, false)).transpose()?,
            to: nonempty(&q.to).map(|v| parse_bound("to", &v, true)).transpose()?,
            session: nonempty(&q.session_id),
            exclude: q.exclude_sessions.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        })
    }
}

/// Ids of the user's observation windows that pass the filters. The inner
/// join drops windows whose observation was deleted or forgotten.
fn allowed_windows(conn: &Connection, user_id: &str, f: &Filters) -> rusqlite::Result<HashSet<String>> {
    let mut sql = format!(
        "SELECT c.id FROM memory_index_chunks c
         JOIN memory_observations o ON o.id = c.source_id AND o.user_id = c.scope
         WHERE c.source_type = 'observation' AND c.scope = ?1 AND {NOT_EPHEMERAL}"
    );
    let mut args: Vec<String> = vec![user_id.to_string()];
    let mut clauses: Vec<(String, String)> = Vec::new();
    if let Some(b) = &f.from {
        clauses.push((format!("datetime(o.timestamp) {}", b.op), b.at.clone()));
    }
    if let Some(b) = &f.to {
        clauses.push((format!("datetime(o.timestamp) {}", b.op), b.at.clone()));
    }
    if let Some(s) = &f.session {
        clauses.push(("o.session_id =".to_string(), s.clone()));
    }
    for (lhs, v) in clauses {
        args.push(v);
        sql.push_str(&format!(" AND {lhs} ?{}", args.len()));
    }
    if !f.exclude.is_empty() {
        let first = args.len() + 1;
        let ph = (0..f.exclude.len()).map(|i| format!("?{}", first + i)).collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND (o.session_id IS NULL OR o.session_id NOT IN ({ph}))"));
        args.extend(f.exclude.iter().cloned());
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?;
    let ids: HashSet<String> = rows.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// One candidate window from hybrid search, in RRF order.
#[derive(Debug, Clone)]
struct Candidate {
    window_id: String,
    observation_id: String,
    text: String,
    fused: f64,
}

fn candidates(
    conn: &Connection,
    user_id: &str,
    query: &str,
    f: &Filters,
    query_embedding: Option<(&str, &[f32])>,
    n: usize,
) -> rusqlite::Result<Vec<Candidate>> {
    let allowed = allowed_windows(conn, user_id, f)?;
    if allowed.is_empty() {
        return Ok(vec![]);
    }
    let scope = Scope { scope: user_id, target_types: &["chunk"], only_ids: Some(&allowed) };
    let hits = memory_index::hybrid_search(conn, &scope, query, query_embedding, n)?;
    if hits.is_empty() {
        return Ok(vec![]);
    }
    let ph = (1..=hits.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT id, source_id, text FROM memory_index_chunks WHERE source_type = 'observation' AND id IN ({ph})"
    ))?;
    let rows = stmt.query_map(params_from_iter(hits.iter().map(|h| h.target_id.as_str())), |r| {
        Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
    })?;
    let mut by_id: HashMap<String, (String, String)> = rows.collect::<rusqlite::Result<_>>()?;
    Ok(hits
        .into_iter()
        .filter_map(|h| {
            by_id.remove(&h.target_id).map(|(observation_id, text)| Candidate {
                window_id: h.target_id,
                observation_id,
                text,
                fused: h.score,
            })
        })
        .collect())
}

/// Most knowledge hits returned.
pub const KNOWLEDGE_TOP: usize = 3;

/// Top [`KNOWLEDGE_TOP`] approved knowledge bullets for `query`, from a search
/// of their own (never the turn windows). One hit per bullet; `path` is the
/// drive file, `kind` is `domain` or `data_access`.
pub fn knowledge_hits(
    conn: &Connection,
    user_id: &str,
    query: &str,
    query_embedding: Option<(&str, &[f32])>,
) -> rusqlite::Result<Vec<KnowledgeHit>> {
    let mut stmt = conn.prepare(
        "SELECT e.fact_id, e.file_path FROM memory_drive_entries e
         JOIN memory_drives d ON d.id = e.drive_id
         JOIN memory_facts f ON f.id = e.fact_id
         WHERE d.kind = 'personal' AND d.user_id = ?1 AND f.user_id = ?1 AND f.valid_until IS NULL
           AND (e.file_path LIKE 'knowledge/domain/%' OR e.file_path LIKE 'knowledge/data-access/%')",
    )?;
    let rows: HashMap<String, String> = stmt
        .query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let allowed: HashSet<String> = rows.keys().cloned().collect();
    let scope = Scope { scope: user_id, target_types: &["fact"], only_ids: Some(&allowed) };
    let hits = memory_index::hybrid_search(conn, &scope, query, query_embedding, KNOWLEDGE_TOP)?;
    let mut out = Vec::new();
    for h in hits {
        let Some(path) = rows.get(&h.target_id) else { continue };
        let text: String = conn.query_row("SELECT fact FROM memory_facts WHERE id = ?1", params![h.target_id], |r| r.get(0))?;
        let kind = if path.starts_with("knowledge/data-access/") { "data_access" } else { "domain" };
        out.push(KnowledgeHit { path: path.clone(), kind: kind.into(), text, score: h.score });
    }
    Ok(out)
}

/// `POST {sidecar}/v1/rerank` (CONTRACT.md "Sidecar") on the embed client's
/// HTTP client and base URL. Returns (document index, relevance), best first.
/// Errors on a disabled sidecar, timeout, non-2xx (503 = reranker off) or a
/// malformed body.
pub async fn rerank(
    index: &EmbedClient,
    query: &str,
    documents: &[String],
    top_n: usize,
    timeout: Duration,
) -> Result<Vec<(usize, f64)>, String> {
    let base = index.url().ok_or("rerank: sidecar disabled")?;
    if documents.is_empty() {
        return Ok(vec![]);
    }
    let v = tokio::time::timeout(timeout, rerank_call(index, base, query, documents, top_n, timeout))
        .await
        .map_err(|_| "rerank timed out".to_string())??;
    let results = v["results"].as_array().ok_or("rerank response has no results")?;
    let mut seen = HashSet::new();
    let mut out: Vec<(usize, f64)> = results
        .iter()
        .filter_map(|r| Some((r["index"].as_u64()? as usize, r["relevance_score"].as_f64()?)))
        .filter(|(i, s)| *i < documents.len() && s.is_finite() && seen.insert(*i))
        .collect();
    if out.is_empty() {
        return Err("rerank returned no usable results".into());
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

async fn rerank_call(
    index: &EmbedClient,
    base: &str,
    query: &str,
    documents: &[String],
    top_n: usize,
    timeout: Duration,
) -> Result<Value, String> {
    let resp = index
        .http()
        .post(format!("{base}/v1/rerank"))
        .timeout(timeout)
        .json(&json!({ "query": query, "documents": documents, "top_n": top_n }))
        .send()
        .await
        .map_err(|e| format!("rerank request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("rerank returned {}", resp.status()));
    }
    resp.json::<Value>().await.map_err(|e| format!("rerank response: {e}"))
}

fn speaker(kind: &str, content: &str) -> String {
    match kind {
        "turn_user" => "user".into(),
        "turn_assistant" => "assistant".into(),
        "turn_tool" => {
            let name = content.lines().next().map(str::trim).filter(|l| {
                !l.is_empty() && l.chars().count() <= 64 && l.chars().all(|c| c.is_alphanumeric() || "_-.:/".contains(c))
            });
            format!("tool:{}", name.unwrap_or("tool"))
        }
        other => other.trim_start_matches("turn_").to_string(),
    }
}

/// `text` clipped at [`TURN_CLIP_CHARS`], marked with [`CLIP_MARK`].
pub fn clip_turn(text: &str) -> String {
    if text.chars().count() <= TURN_CLIP_CHARS {
        text.to_string()
    } else {
        format!("{}{CLIP_MARK}", text.chars().take(TURN_CLIP_CHARS).collect::<String>())
    }
}

fn turn_from_row(r: &Row) -> rusqlite::Result<RawTurn> {
    let kind: String = r.get(2)?;
    let content: String = r.get::<_, Option<String>>(5)?.unwrap_or_default();
    Ok(RawTurn {
        observation_id: r.get(0)?,
        session_id: r.get(1)?,
        speaker: speaker(&kind, &content),
        timestamp: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        source: r.get(4)?,
        text: clip_turn(&content),
        score: 0.0,
        window_ids: vec![],
    })
}

fn load_turns(conn: &Connection, user_id: &str, ids: &[String]) -> rusqlite::Result<HashMap<String, RawTurn>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ph = (2..ids.len() + 2).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let mut stmt =
        conn.prepare(&format!("SELECT {TURN_COLS} FROM memory_observations o WHERE o.user_id = ?1 AND {NOT_EPHEMERAL} AND o.id IN ({ph})"))?;
    let args = std::iter::once(user_id).chain(ids.iter().map(String::as_str));
    let rows = stmt.query_map(params_from_iter(args), turn_from_row)?;
    let turns: HashMap<String, RawTurn> = rows.map(|t| t.map(|t| (t.observation_id.clone(), t))).collect::<rusqlite::Result<_>>()?;
    Ok(turns)
}

/// Raw past-chat search for `user_id` (see the module docs).
pub async fn search(db: &DbHandle, index: &EmbedClient, user_id: &str, q: RawQuery) -> Result<RawResult, RawError> {
    let filters = Filters::of(&q)?;
    let budget = q.mode.budget();
    let top = q.limit.unwrap_or(budget.top).clamp(1, MAX_LIMIT);
    let query = q.query.trim().to_string();
    if query.is_empty() || user_id.is_empty() {
        return Ok(RawResult::default());
    }

    let emb = index.embed_or_hash(&[query.clone()], InputType::Query).await;
    let (db2, user, q2, n, want_knowledge) = (db.clone(), user_id.to_string(), query.clone(), budget.candidates.max(top), q.include_knowledge);
    let (cands, knowledge) = tokio::task::spawn_blocking(move || -> rusqlite::Result<(Vec<Candidate>, Vec<KnowledgeHit>)> {
        let conn = db2.connect()?;
        let vector = emb.vectors.first().filter(|v| !v.is_empty()).map(|v| (emb.model.as_str(), v.as_slice()));
        let knowledge = if want_knowledge { knowledge_hits(&conn, &user, &q2, vector)? } else { vec![] };
        Ok((candidates(&conn, &user, &q2, &filters, vector, n)?, knowledge))
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    if cands.is_empty() {
        return Ok(RawResult { knowledge, ..RawResult::default() });
    }

    let docs: Vec<String> = cands.iter().map(|c| c.text.clone()).collect();
    let (ranked, reranked) = match rerank(index, &query, &docs, top, budget.rerank_timeout).await {
        Ok(r) => (r, true),
        Err(e) => {
            tracing::debug!(error = %e, "raw search: rerank unavailable, keeping fused order");
            (cands.iter().enumerate().map(|(i, c)| (i, c.fused)).collect(), false)
        }
    };

    // Group the top windows by observation; the first window seen is the best.
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, (f64, Vec<String>)> = HashMap::new();
    for (i, score) in ranked.into_iter().take(top) {
        let c = &cands[i];
        match groups.get_mut(&c.observation_id) {
            Some(g) => g.1.push(c.window_id.clone()),
            None => {
                order.push(c.observation_id.clone());
                groups.insert(c.observation_id.clone(), (score, vec![c.window_id.clone()]));
            }
        }
    }

    let (db2, user, ids) = (db.clone(), user_id.to_string(), order.clone());
    let mut loaded = tokio::task::spawn_blocking(move || db2.connect().and_then(|c| load_turns(&c, &user, &ids)))
        .await
        .map_err(internal)?
        .map_err(internal)?;

    let max_chars = budget.tokens * CHARS_PER_TOKEN;
    let mut used = 0usize;
    let mut turns = Vec::new();
    for id in order {
        let Some(mut t) = loaded.remove(&id) else { continue };
        let len = t.text.chars().count();
        if used + len > max_chars {
            break;
        }
        used += len;
        if let Some((score, windows)) = groups.remove(&id) {
            t.score = score;
            t.window_ids = windows;
        }
        turns.push(t);
    }
    Ok(RawResult { turns, knowledge, reranked })
}

/// [`search`] for a caller that may be in an incognito session: an ephemeral
/// `calling_session` gets an empty result (no turns, no knowledge), so an
/// incognito chat never has past chats pulled into it, whatever the client
/// believes about its own mode.
pub async fn search_for_session(
    db: &DbHandle,
    index: &EmbedClient,
    user_id: &str,
    calling_session: Option<&str>,
    q: RawQuery,
) -> Result<RawResult, RawError> {
    if let Some(session) = calling_session.map(str::trim).filter(|s| !s.is_empty()) {
        // A failed lookup is treated as ephemeral: never leak into a chat we cannot vouch for.
        if db.is_session_ephemeral(session).unwrap_or(true) {
            return Ok(RawResult::default());
        }
    }
    search(db, index, user_id, q).await
}

/// Same-session turns around `observation_id`, in time order (the turn
/// itself included). `None` when the observation does not exist, is not the
/// user's, or is in an incognito session.
pub fn neighbors(
    conn: &Connection,
    user_id: &str,
    observation_id: &str,
    before: usize,
    after: usize,
) -> rusqlite::Result<Option<Vec<RawTurn>>> {
    let (before, after) = (before.min(MAX_NEIGHBORS), after.min(MAX_NEIGHBORS));
    let anchor = conn
        .query_row(
            &format!(
                "SELECT {TURN_COLS}, o.rowid, COALESCE(datetime(o.timestamp), '') FROM memory_observations o
                 WHERE o.id = ?1 AND o.user_id = ?2 AND {NOT_EPHEMERAL}"
            ),
            params![observation_id, user_id],
            |r| Ok((turn_from_row(r)?, r.get::<_, i64>(6)?, r.get::<_, String>(7)?)),
        )
        .optional()?;
    let Some((turn, rowid, at)) = anchor else { return Ok(None) };
    let Some(session) = turn.session_id.clone() else { return Ok(Some(vec![turn])) };

    let side = |cmp: &str, dir: &str, n: usize| -> rusqlite::Result<Vec<RawTurn>> {
        if n == 0 {
            return Ok(vec![]);
        }
        let sql = format!(
            "SELECT {TURN_COLS} FROM memory_observations o
             WHERE o.user_id = ?1 AND o.session_id = ?2 AND o.kind LIKE 'turn\\_%' ESCAPE '\\'
               AND (COALESCE(datetime(o.timestamp), ''), o.rowid) {cmp} (?3, ?4)
             ORDER BY COALESCE(datetime(o.timestamp), '') {dir}, o.rowid {dir} LIMIT ?5"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![user_id, session, at, rowid, n as i64], turn_from_row)?;
        let turns: Vec<RawTurn> = rows.collect::<rusqlite::Result<_>>()?;
        Ok(turns)
    };
    let mut out = side("<", "DESC", before)?;
    out.reverse();
    out.push(turn);
    out.extend(side(">", "ASC", after)?);
    Ok(Some(out))
}

/// Activity line for a past-chat item: `"<Mon D> · <first 60 chars>"`.
pub fn past_chat_title(t: &RawTurn) -> String {
    let date = chrono::DateTime::parse_from_rfc3339(&t.timestamp)
        .map(|d| d.format("%b %-d").to_string())
        .unwrap_or_else(|_| "Earlier".to_string());
    let flat = t.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let head: String = flat.chars().take(60).collect();
    let more = if flat.chars().count() > 60 { "…" } else { "" };
    format!("{date} · {head}{more}")
}

/// Record the turns a reply was given as one `recalled` activity row for the
/// calling session, items typed `past_chat`. Best effort; incognito sessions
/// are skipped by `memory_activity::record`.
pub fn record_past_chats(conn: &Connection, owner: &str, session: &str, turns: &[RawTurn]) -> Option<i64> {
    if turns.is_empty() || session.is_empty() {
        return None;
    }
    let n = turns.len();
    let items = turns
        .iter()
        .map(|t| (past_chat_title(t), t.source.clone().unwrap_or_else(|| "past_chat".to_string())))
        .collect();
    let refs: Vec<Value> = turns
        .iter()
        .map(|t| json!({ "id": t.observation_id, "title": past_chat_title(t), "source": "past_chat", "date": t.timestamp, "session_id": t.session_id }))
        .collect();
    crate::memory_activity::record(
        conn,
        owner,
        crate::memory_activity::Activity {
            kind: "recalled",
            title: format!("Looked at {n} {} from past chats", if n == 1 { "turn" } else { "turns" }),
            items,
            extra: Some(json!({ "item_type": "past_chat", "refs": refs })),
            session: Some(session.to_string()),
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_kernel_service as kernel;

    const USER: &str = "u1";

    fn disabled() -> EmbedClient {
        EmbedClient::new(None, memory_index::DEFAULT_EMBED_MODEL)
    }

    /// Port 9 (discard) is never served locally: connection refused.
    fn down() -> EmbedClient {
        EmbedClient::new(Some("http://127.0.0.1:9".into()), memory_index::DEFAULT_EMBED_MODEL)
    }

    fn toks(s: &str) -> HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric()).filter(|t| t.len() > 2).map(|t| t.to_lowercase()).collect()
    }

    /// Mock sidecar: only `/v1/rerank` (scores = query-token overlap), after
    /// `delay`. `/v1/embeddings` is 404, so queries take the hash fallback.
    async fn sidecar(delay: Duration) -> EmbedClient {
        use axum::{routing::post, Json, Router};
        let app = Router::new().route(
            "/v1/rerank",
            post(move |Json(body): Json<Value>| async move {
                tokio::time::sleep(delay).await;
                let q = toks(body["query"].as_str().unwrap_or(""));
                let mut results: Vec<Value> = body["documents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(i, d)| {
                        let d = toks(d.as_str().unwrap_or(""));
                        json!({ "index": i, "relevance_score": q.intersection(&d).count() as f64 / (q.len().max(1) as f64) })
                    })
                    .collect();
                results.sort_by(|a, b| b["relevance_score"].as_f64().partial_cmp(&a["relevance_score"].as_f64()).unwrap());
                if let Some(n) = body["top_n"].as_u64() {
                    results.truncate(n as usize);
                }
                Json(json!({ "model": "mock-reranker", "results": results }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        EmbedClient::new(Some(format!("http://{addr}")), memory_index::DEFAULT_EMBED_MODEL)
    }

    fn turn(db: &DbHandle, user: &str, session: &str, kind: &str, text: &str, at: &str) -> String {
        let id = kernel::record_observation(db, user, None, Some(session), kind, text, Some("gizzi:test")).unwrap();
        db.connect().unwrap().execute("UPDATE memory_observations SET timestamp = ?1 WHERE id = ?2", params![at, id]).unwrap();
        id
    }

    /// Chunk turns into windows and hash-embed them, as the indexer does.
    async fn index(db: &DbHandle) {
        for _ in 0..4 {
            memory_index::index_pending(db, &disabled(), 512).await.unwrap();
        }
    }

    /// Twenty distractor turns over several sessions plus one answer turn.
    async fn fixture(db: &DbHandle) -> String {
        let filler = [
            "Can you help me plan a birthday dinner for my sister on Saturday?",
            "The quarterly invoice for the Acme account went out on Tuesday.",
            "I prefer dark roast coffee beans, ideally from Ethiopia.",
            "Let's move the standup to 9:30 so the London team can join.",
            "My running shoes are worn out after about five hundred miles.",
            "The kitchen faucet keeps dripping even after the washer swap.",
            "Please draft a polite reply declining the conference invite.",
            "Our dog needs his rabies booster before the end of the month.",
            "The Android build failed because the signing key was missing.",
            "Book a table for four at the Thai place near the office.",
            "I am reading a history of the Roman republic this winter.",
            "The garden tomatoes finally ripened after the warm week.",
            "Summarize the lease renewal terms the landlord sent over.",
            "My flight to Denver lands at 6:40 in the evening on Friday.",
            "We should rotate the API tokens for the staging cluster.",
            "The piano tuner is coming on Wednesday afternoon.",
            "Can you compare two electric kettles for under fifty dollars?",
            "I switched my phone plan to the cheaper family bundle.",
            "The marketing deck still needs the new pricing slide.",
            "Remind me to renew my passport before the spring trip.",
        ];
        for (i, f) in filler.iter().enumerate() {
            let kind = if i % 2 == 0 { "turn_user" } else { "turn_assistant" };
            turn(db, USER, &format!("ses_{}", i % 5), kind, f, &format!("2026-11-{:02} 10:00:00", 1 + i));
        }
        let answer = turn(
            db,
            USER,
            "ses_rerank",
            "turn_assistant",
            "We picked bge-reranker-v2-m3 for reranking the past chat windows because it runs on the local sidecar.",
            "2026-11-20 14:05:00",
        );
        index(db).await;
        answer
    }

    fn q(query: &str) -> RawQuery {
        RawQuery { query: query.into(), ..Default::default() }
    }

    #[tokio::test]
    async fn recall_at_8_finds_the_answer_turn_with_rerank() {
        let db = DbHandle::new_memory().unwrap();
        let answer = fixture(&db).await;
        let side = sidecar(Duration::ZERO).await;
        let r = search(&db, &side, USER, q("which reranker model did we pick for reranking?")).await.unwrap();
        assert!(r.reranked);
        assert!(r.knowledge.is_empty());
        let ids: Vec<&str> = r.turns.iter().take(8).map(|t| t.observation_id.as_str()).collect();
        assert!(ids.contains(&answer.as_str()), "answer not in top 8: {ids:?}");
        let hit = r.turns.iter().find(|t| t.observation_id == answer).unwrap();
        assert_eq!(hit.speaker, "assistant");
        assert_eq!(hit.session_id.as_deref(), Some("ses_rerank"));
        assert_eq!(hit.timestamp, "2026-11-20T14:05:00Z");
        assert!(hit.text.starts_with("We picked bge-reranker"), "turn text is the raw turn, not the window: {}", hit.text);
        assert!(!hit.window_ids.is_empty());
        // Auto mode keeps at most 8 windows.
        let auto = search(&db, &side, USER, RawQuery { mode: RawMode::Auto, ..q("reranker") }).await.unwrap();
        assert!(auto.turns.len() <= 8);
        assert!(auto.turns.iter().any(|t| t.observation_id == answer));
    }

    #[tokio::test]
    async fn sidecar_down_or_slow_keeps_fused_order() {
        let db = DbHandle::new_memory().unwrap();
        let answer = fixture(&db).await;
        let query = "which reranker model did we pick for reranking?";
        let r = search(&db, &down(), USER, q(query)).await.unwrap();
        assert!(!r.reranked);
        assert!(r.turns.iter().any(|t| t.observation_id == answer));
        // Fused order = the first window of each observation in RRF order.
        let emb = memory_index::hash_embed(&[query.to_string()]);
        let conn = db.connect().unwrap();
        let fused = candidates(&conn, USER, query, &Filters::default(), Some((emb.model.as_str(), emb.vectors[0].as_slice())), 128).unwrap();
        let mut expected: Vec<String> = Vec::new();
        for c in fused.iter().take(12) {
            if !expected.contains(&c.observation_id) {
                expected.push(c.observation_id.clone());
            }
        }
        let got: Vec<String> = r.turns.iter().map(|t| t.observation_id.clone()).collect();
        assert_eq!(got, expected);
        assert!(r.turns.windows(2).all(|w| w[0].score >= w[1].score));

        // A sidecar slower than the auto timeout (600 ms) also falls back.
        let slow = sidecar(Duration::from_millis(1500)).await;
        let started = std::time::Instant::now();
        let r = search(&db, &slow, USER, RawQuery { mode: RawMode::Auto, ..q(query) }).await.unwrap();
        assert!(!r.reranked);
        assert!(started.elapsed() < Duration::from_millis(1400), "timeout not applied: {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn incognito_sessions_never_come_back() {
        let db = DbHandle::new_memory().unwrap();
        let open = turn(&db, USER, "ses_open", "turn_user", "the zanzibar ferry schedule changed", "2026-11-10 09:00:00");
        db.set_session_ephemeral("ses_inc").unwrap();
        let secret = turn(&db, USER, "ses_inc", "turn_user", "the zanzibar ferry secret plan", "2026-11-10 09:01:00");
        index(&db).await;
        for query in [q("zanzibar ferry"), RawQuery { session_id: Some("ses_inc".into()), ..q("zanzibar ferry") }] {
            let r = search(&db, &down(), USER, query).await.unwrap();
            assert!(r.turns.iter().all(|t| t.observation_id != secret));
        }
        let r = search(&db, &down(), USER, q("zanzibar ferry")).await.unwrap();
        assert!(r.turns.iter().any(|t| t.observation_id == open));
        assert!(neighbors(&db.connect().unwrap(), USER, &secret, 2, 2).unwrap().is_none());
    }

    #[tokio::test]
    async fn from_to_filter_on_observation_time() {
        let db = DbHandle::new_memory().unwrap();
        let early = turn(&db, USER, "s1", "turn_user", "glacier hiking boots order", "2026-10-15 08:00:00");
        let mid = turn(&db, USER, "s2", "turn_user", "glacier hiking boots return", "2026-11-05 23:59:00");
        let late = turn(&db, USER, "s3", "turn_user", "glacier hiking boots review", "2026-11-25 12:00:00");
        index(&db).await;
        let ids = |r: &RawResult| r.turns.iter().map(|t| t.observation_id.clone()).collect::<HashSet<_>>();
        let r = search(&db, &down(), USER, RawQuery { from: Some("2026-11-01".into()), to: Some("2026-11-05".into()), ..q("glacier boots") })
            .await
            .unwrap();
        assert_eq!(ids(&r), HashSet::from([mid.clone()]));
        let r = search(&db, &down(), USER, RawQuery { from: Some("2026-11-06T00:00:00Z".into()), ..q("glacier boots") }).await.unwrap();
        assert_eq!(ids(&r), HashSet::from([late.clone()]));
        let r = search(&db, &down(), USER, RawQuery { to: Some("2026-10-31".into()), ..q("glacier boots") }).await.unwrap();
        assert_eq!(ids(&r), HashSet::from([early]));
        let r = search(&db, &down(), USER, RawQuery { exclude_sessions: vec!["s2".into(), "s3".into()], ..q("glacier boots") }).await.unwrap();
        assert!(!ids(&r).contains(&mid) && !ids(&r).contains(&late));
        let bad = search(&db, &down(), USER, RawQuery { from: Some("last tuesday".into()), ..q("glacier") }).await;
        assert!(matches!(bad, Err(RawError::BadRequest(_))));
    }

    #[tokio::test]
    async fn windows_of_deleted_observations_are_dropped() {
        let db = DbHandle::new_memory().unwrap();
        let keep = turn(&db, USER, "s1", "turn_user", "orchid repotting soil mix", "2026-11-10 09:00:00");
        let gone = turn(&db, USER, "s1", "turn_user", "orchid repotting bark chips", "2026-11-10 09:01:00");
        index(&db).await;
        let conn = db.connect().unwrap();
        conn.execute("DELETE FROM memory_observations WHERE id = ?1", params![gone]).unwrap();
        let r = search(&db, &down(), USER, q("orchid repotting")).await.unwrap();
        assert!(r.turns.iter().any(|t| t.observation_id == keep));
        assert!(r.turns.iter().all(|t| t.observation_id != gone));
    }

    #[tokio::test]
    async fn budget_caps_total_chars() {
        let db = DbHandle::new_memory().unwrap();
        for i in 0..12 {
            let body = format!("nebula telescope mirror grinding notes {i}. {}", "polish the mirror slowly and evenly. ".repeat(80));
            turn(&db, USER, &format!("s{i}"), "turn_user", &body, &format!("2026-11-{:02} 09:00:00", i + 1));
        }
        index(&db).await;
        for mode in [RawMode::Search, RawMode::Auto] {
            let r = search(&db, &down(), USER, RawQuery { mode, ..q("nebula telescope mirror") }).await.unwrap();
            assert!(!r.turns.is_empty());
            let total: usize = r.turns.iter().map(|t| t.text.chars().count()).sum();
            assert!(total <= mode.budget().tokens * CHARS_PER_TOKEN, "{mode:?}: {total} chars");
            for t in &r.turns {
                assert!(t.text.ends_with(CLIP_MARK));
                assert_eq!(t.text.chars().count(), TURN_CLIP_CHARS + CLIP_MARK.chars().count());
            }
        }
    }

    #[tokio::test]
    async fn neighbors_in_time_order_and_owner_checked() {
        let db = DbHandle::new_memory().unwrap();
        let a = turn(&db, USER, "s1", "turn_user", "first", "2026-11-10 09:00:00");
        let b = turn(&db, USER, "s1", "turn_assistant", "second", "2026-11-10 09:01:00");
        let c = turn(&db, USER, "s1", "turn_user", "third", "2026-11-10 09:02:00");
        let d = turn(&db, USER, "s1", "turn_assistant", "fourth", "2026-11-10 09:03:00");
        let _e = turn(&db, USER, "s1", "turn_user", "fifth", "2026-11-10 09:04:00");
        let _other_session = turn(&db, USER, "s2", "turn_user", "elsewhere", "2026-11-10 09:01:30");
        let theirs = turn(&db, "u2", "s9", "turn_user", "not yours", "2026-11-10 09:00:00");
        let conn = db.connect().unwrap();
        let got = neighbors(&conn, USER, &c, 2, 1).unwrap().unwrap();
        let ids: Vec<&str> = got.iter().map(|t| t.observation_id.as_str()).collect();
        assert_eq!(ids, vec![a.as_str(), b.as_str(), c.as_str(), d.as_str()]);
        assert_eq!(got[1].speaker, "assistant");
        assert!(neighbors(&conn, USER, &theirs, 2, 2).unwrap().is_none());
        assert!(neighbors(&conn, USER, "obs_missing", 2, 2).unwrap().is_none());
    }

    #[test]
    fn past_chat_activity_title_and_row() {
        let db = DbHandle::new_memory().unwrap();
        let t = RawTurn {
            observation_id: "obs_1".into(),
            session_id: Some("ses_old".into()),
            speaker: "user".into(),
            timestamp: "2026-11-20T14:05:00Z".into(),
            source: Some("gizzi:test".into()),
            text: "we picked bge for reranking ".repeat(5),
            score: 0.9,
            window_ids: vec![],
        };
        let title = past_chat_title(&t);
        assert!(title.starts_with("Nov 20 · we picked bge for reranking"));
        assert!(title.ends_with('…'));
        let conn = db.connect().unwrap();
        assert!(record_past_chats(&conn, USER, "ses_now", &[t]).is_some());
        let (kind, detail): (String, String) = conn
            .query_row("SELECT kind, detail FROM memory_activity WHERE user_id = ?1 AND session_id = 'ses_now'", params![USER], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kind, "recalled");
        let detail: Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(detail["item_type"], "past_chat");
        assert_eq!(detail["refs"][0]["id"], "obs_1");
        assert!(detail["items"][0]["text"].as_str().unwrap().starts_with("Nov 20 · "));
    }

    #[tokio::test]
    async fn approved_knowledge_files_come_back_in_knowledge_never_in_turns() {
        use crate::memory_drive::{Entry, Operation};
        use crate::{memory_drive_service as service, memory_drive_writer as writer};
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let db = DbHandle::new(base.join("t.db")).unwrap();
        writer::configure_root(&db, &base.join("brains")).unwrap();
        let drive = writer::ensure(&db, USER).unwrap().unwrap();
        let line = |id: &str, text: &str, path: &str, kind: &str| Operation::UpsertEntry {
            path: path.into(),
            entry: Entry {
                id: id.into(),
                text: text.into(),
                source: "knowledge:proposal/kprop_1".into(),
                added: chrono::Utc::now().format("%Y-%m-%d").to_string(),
                metadata: [("kind".to_string(), kind.to_string())].into(),
            },
        };
        let rev = drive.storage().unwrap().snapshot(None).unwrap().revision;
        service::apply(
            &db,
            USER,
            &rev,
            &[
                line("k1", "Invoices live in the billing warehouse table fct_invoices, joined on account_id.", "knowledge/data-access/billing.md", "data_access"),
                line("k2", "A churned account means no paid invoice for ninety days.", "knowledge/domain/definitions.md", "domain"),
                line("k3", "User likes oat milk.", "facts.md", "fact"),
            ],
            "approve knowledge",
        )
        .unwrap();
        // A raw turn that talks about the same thing stays a turn.
        let t = turn(&db, USER, "ses_1", "turn_user", "Where do the invoices table and account_id join live in the warehouse?", "2026-11-20 10:00:00");
        index(&db).await;

        let query = RawQuery { include_knowledge: true, ..q("which warehouse table holds invoices joined on account_id?") };
        let r = search(&db, &down(), USER, query.clone()).await.unwrap();
        assert!(r.knowledge.len() <= KNOWLEDGE_TOP);
        let top = r.knowledge.first().expect("a knowledge hit");
        assert_eq!(top.path, "knowledge/data-access/billing.md");
        assert_eq!(top.kind, "data_access");
        assert!(top.text.starts_with("Invoices live in the billing warehouse table"));
        assert!(r.knowledge.iter().all(|k| k.path.starts_with("knowledge/")), "ordinary facts are not knowledge: {:?}", r.knowledge);
        assert!(r.turns.iter().any(|x| x.observation_id == t));
        assert!(r.turns.iter().all(|x| !x.text.contains("fct_invoices")), "knowledge never lands in turns");

        // Off by default, and another user sees none of it.
        assert!(search(&db, &down(), USER, q(&query.query)).await.unwrap().knowledge.is_empty());
        assert!(search(&db, &down(), "u2", query).await.unwrap().knowledge.is_empty());

        // Removing the bullet from the drive retires it.
        let rev = service::read(&db, USER, None).unwrap().revision;
        service::apply(&db, USER, &rev, &[Operation::DeleteEntry { id: "k1".into() }], "rollback").unwrap();
        let again = RawQuery { include_knowledge: true, ..q("which warehouse table holds invoices joined on account_id?") };
        assert!(search(&db, &down(), USER, again).await.unwrap().knowledge.iter().all(|k| k.kind != "data_access"));
    }

    #[tokio::test]
    async fn an_ephemeral_calling_session_gets_an_empty_result() {
        let db = DbHandle::new_memory().unwrap();
        let answer = fixture(&db).await;
        db.connect().unwrap().execute("INSERT INTO ephemeral_sessions (session_id) VALUES ('ses_incognito')", []).unwrap();
        let query = RawQuery { include_knowledge: true, ..q("which reranker model did we pick for reranking?") };
        let r = search_for_session(&db, &down(), USER, Some("ses_incognito"), query.clone()).await.unwrap();
        assert!(r.turns.is_empty() && r.knowledge.is_empty() && !r.reranked);
        // The same query from an ordinary session still finds the turn.
        let ok = search_for_session(&db, &down(), USER, Some("ses_normal"), query.clone()).await.unwrap();
        assert!(ok.turns.iter().any(|t| t.observation_id == answer));
        let none = search_for_session(&db, &down(), USER, None, query).await.unwrap();
        assert!(none.turns.iter().any(|t| t.observation_id == answer));
    }
}
