//! Memory consolidation + store adapters (WP-M1d, decision M1).
//!
//! **Consolidation** is a background batch job over the canonical memory
//! kernel (`memory_facts`), per user, bounded per run, idempotent (it only
//! looks at still-valid facts, so a second run finds nothing left to merge):
//! - **Merge duplicates.** Candidate pairs are facts of the same user and
//!   agent whose normalised text is equal, that an earlier edge already
//!   labelled `same`, or that overlap strongly (token Jaccard ≥
//!   [`NEAR_JACCARD`]). The S1 RELATION bank judges every pair in shadow; the
//!   **incumbent rule** (equal normalised text or an existing `same` edge)
//!   decides live until the bank passes Q26 and is listed in
//!   `ALLTERNIT_S1_LIVE_BANKS` (then S1's `same` at confidence ≥
//!   [`S1_LIVE_MIN_CONFIDENCE`] decides, falling back to the incumbent when
//!   S1 is unreachable). A merge is soft: the duplicate gets `valid_until`
//!   and a `same` edge onto the survivor; nothing is deleted.
//! - **Decay.** Facts older than [`DECAY_AGE_DAYS`], never retrieved, with
//!   confidence below [`DECAY_MAX_CONFIDENCE`] get `decayed_at` and a
//!   `decay_score`; recall down-weights them and a later retrieval revives
//!   them. Never a delete.
//!
//! The job makes no S2 model calls (S1 decisions go to the local decision
//! runtime), so there is nothing for the internal batch path to carry; S1
//! decisions and the run itself are written to the usage ledger (surface
//! `memory`).
//!
//! **Store adapters.** Other memory stores (gizzi memdir/brain, the
//! memory-agent service) write through `/api/v1/memory/adapters/*`: each
//! external item maps to one canonical fact (`memory_adapter_links`), so a
//! re-send is an update and a one-time import is idempotent. Notes and
//! procedures live in this database already and join the shared index (V209).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::db::DbHandle;
use crate::memory_relations::{self as rel, MemoryType, NodeKind, RelationType, S1Answer, S1Client, BANK_RELATION};
use crate::usage_ledger::{self as ledger, LedgerCtx, LedgerRow};
use crate::{auth::AuthUser, AppState};

/// Facts scanned per user per run (oldest first; the rest wait a run).
pub const SCAN_LIMIT: usize = 400;
/// Merges applied per user per run.
pub const MAX_MERGES: usize = 50;
/// S1 shadow decisions per user per run.
pub const MAX_SHADOW: usize = 40;
pub const NEAR_JACCARD: f64 = 0.6;
pub const S1_LIVE_MIN_CONFIDENCE: f64 = 0.8;
pub const DECAY_AGE_DAYS: i64 = 90;
pub const DECAY_MAX_CONFIDENCE: f64 = 0.5;
pub const MAX_DECAYS: usize = 200;
/// A user is due again this long after their last run.
pub const RUN_EVERY: Duration = Duration::from_secs(24 * 3600);
pub const NODE: &str = "memory.consolidation";

const CONSOLIDATE_INSTRUCTIONS: &str = "Do these two memories about the user say the same thing? \
same = one restates the other; updates = the second replaces the first with a newer value; \
contradicts = they cannot both be true; unrelated = different things.";

// ─── Consolidation ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FactRow {
    pub id: String,
    pub agent_id: Option<String>,
    pub fact: String,
    pub confidence: f64,
    pub valid_from: String,
    pub retrieval_count: i64,
}

/// Lowercased alphanumeric tokens joined by one space.
pub fn normalize(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_lowercase).collect::<Vec<_>>().join(" ")
}

fn token_set(text: &str) -> HashSet<String> {
    normalize(text).split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect()
}

pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    a.intersection(b).count() as f64 / a.union(b).count() as f64
}

/// One candidate duplicate: `drop` would be merged into `keep`.
#[derive(Debug, Clone)]
pub struct Pair {
    pub keep: FactRow,
    pub drop: FactRow,
    /// What the incumbent rule says (true = merge).
    pub incumbent_same: bool,
    pub jaccard: f64,
}

pub fn load_facts(conn: &Connection, user_id: &str, limit: usize) -> rusqlite::Result<Vec<FactRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, fact, confidence, COALESCE(valid_from, ''), retrieval_count FROM memory_facts
         WHERE user_id = ?1 AND valid_until IS NULL AND (memory_type IS NULL OR memory_type != 'not_memory')
         ORDER BY valid_from ASC, id ASC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![user_id, limit as i64], |r| {
        Ok(FactRow {
            id: r.get(0)?,
            agent_id: r.get(1)?,
            fact: r.get(2)?,
            confidence: r.get(3)?,
            valid_from: r.get(4)?,
            retrieval_count: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// Still-valid fact pairs an earlier `same` edge links (either direction).
fn same_edges(conn: &Connection, user_id: &str) -> rusqlite::Result<HashSet<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT source_entity_id, target_entity_id FROM memory_relationships
         WHERE user_id = ?1 AND relation_type = 'same' AND source_kind = 'fact' AND target_kind = 'fact' AND valid_until IS NULL",
    )?;
    let rows = stmt.query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = HashSet::new();
    for r in rows {
        let (a, b) = r?;
        out.insert((b.clone(), a.clone()));
        out.insert((a, b));
    }
    Ok(out)
}

/// The survivor of a pair: higher confidence, then the older fact.
fn order(a: &FactRow, b: &FactRow) -> bool {
    a.confidence > b.confidence || (a.confidence == b.confidence && (a.valid_from.as_str(), a.id.as_str()) <= (b.valid_from.as_str(), b.id.as_str()))
}

/// Candidate pairs among `facts`; each fact is in at most one pair per run.
pub fn candidate_pairs(facts: &[FactRow], edges: &HashSet<(String, String)>) -> Vec<Pair> {
    let norms: Vec<String> = facts.iter().map(|f| normalize(&f.fact)).collect();
    let sets: Vec<HashSet<String>> = facts.iter().map(|f| token_set(&f.fact)).collect();
    let mut used = HashSet::new();
    let mut out = Vec::new();
    for i in 0..facts.len() {
        if used.contains(&i) {
            continue;
        }
        for j in (i + 1)..facts.len() {
            if used.contains(&j) || facts[i].agent_id != facts[j].agent_id {
                continue;
            }
            let exact = !norms[i].is_empty() && norms[i] == norms[j];
            let edge = edges.contains(&(facts[i].id.clone(), facts[j].id.clone()));
            let jac = jaccard(&sets[i], &sets[j]);
            if !(exact || edge || jac >= NEAR_JACCARD) {
                continue;
            }
            let (keep, drop) = if order(&facts[i], &facts[j]) { (&facts[i], &facts[j]) } else { (&facts[j], &facts[i]) };
            out.push(Pair { keep: keep.clone(), drop: drop.clone(), incumbent_same: exact || edge, jaccard: jac });
            used.insert(i);
            used.insert(j);
            break;
        }
    }
    out
}

/// Soft merge of `drop` into `keep`. Returns false when `drop` was already
/// retired (a concurrent run or edit), so re-running is harmless.
pub fn apply_merge(conn: &Connection, user_id: &str, pair: &Pair, origin: &str, decision_id: Option<&str>) -> rusqlite::Result<bool> {
    let retired = conn.execute(
        "UPDATE memory_facts SET valid_until = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
        params![pair.drop.id, user_id],
    )?;
    if retired == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE memory_facts SET confidence = MAX(confidence, ?3), retrieval_count = retrieval_count + ?4,
                decayed_at = CASE WHEN ?4 > 0 THEN NULL ELSE decayed_at END
         WHERE id = ?1 AND user_id = ?2",
        params![pair.keep.id, user_id, pair.drop.confidence, pair.drop.retrieval_count],
    )?;
    rel::write_relation(conn, user_id, (NodeKind::Fact, &pair.drop.id), RelationType::Same, (NodeKind::Fact, &pair.keep.id), 1.0, origin, decision_id)?;
    // Adapter items follow their fact to the survivor.
    conn.execute("UPDATE memory_adapter_links SET fact_id = ?2 WHERE fact_id = ?1 AND user_id = ?3", params![pair.drop.id, pair.keep.id, user_id])?;
    Ok(true)
}

/// Mark stale, never-retrieved, low-confidence facts as decayed (soft).
pub fn decay(conn: &Connection, user_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        &format!(
            "UPDATE memory_facts SET decayed_at = CURRENT_TIMESTAMP, decay_score = confidence * 0.5
             WHERE id IN (SELECT id FROM memory_facts
                          WHERE user_id = ?1 AND valid_until IS NULL AND decayed_at IS NULL AND retrieval_count = 0
                            AND confidence < ?2 AND valid_from < datetime('now', '-{DECAY_AGE_DAYS} days')
                          LIMIT {MAX_DECAYS})"
        ),
        params![user_id, DECAY_MAX_CONFIDENCE],
    )
}

/// Recall saw these facts: count it and revive any that had decayed.
pub fn note_retrieved(conn: &Connection, user_id: &str, fact_ids: &[&str]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "UPDATE memory_facts SET retrieval_count = retrieval_count + 1, last_retrieved_at = CURRENT_TIMESTAMP,
                decayed_at = NULL, decay_score = NULL
         WHERE id = ?1 AND user_id = ?2",
    )?;
    for id in fact_ids {
        stmt.execute(params![id, user_id])?;
    }
    Ok(())
}

/// Whether the S1 RELATION bank serves live (Q26 passed and listed).
pub fn relation_bank_live() -> bool {
    std::env::var("ALLTERNIT_S1_LIVE_BANKS")
        .map(|v| v.split(',').any(|b| b.trim() == BANK_RELATION))
        .unwrap_or(false)
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct RunReport {
    pub run_id: String,
    pub scanned: usize,
    pub candidates: usize,
    pub merged: usize,
    pub shadow_decisions: usize,
    pub outcomes_reported: usize,
    pub decayed: usize,
    pub live_bank: &'static str,
}

fn sql_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// One consolidation run for one user.
pub async fn run_for_user(db: &DbHandle, client: &S1Client, user_id: &str, s1_live: bool) -> Result<RunReport, String> {
    let started = Instant::now();
    let run_id = format!("mcr_{}", Uuid::new_v4().simple());
    let mut report = RunReport { run_id: run_id.clone(), live_bank: if s1_live { "s1" } else { "incumbent" }, ..Default::default() };
    let (facts, pairs) = {
        let conn = db.connect().map_err(sql_err)?;
        conn.execute(
            "INSERT INTO memory_consolidation_runs (id, user_id, live_bank) VALUES (?1, ?2, ?3)",
            params![run_id, user_id, report.live_bank],
        )
        .map_err(sql_err)?;
        let facts = load_facts(&conn, user_id, SCAN_LIMIT).map_err(sql_err)?;
        let edges = same_edges(&conn, user_id).map_err(sql_err)?;
        let pairs = candidate_pairs(&facts, &edges);
        (facts, pairs)
    };
    report.scanned = facts.len();
    report.candidates = pairs.len();
    let labels: Vec<&str> = RelationType::ALL.iter().map(|t| t.as_str()).collect();
    let ctx = LedgerCtx::surface("memory").run(&run_id, Some(NODE)).tenant(None, Some(user_id));

    for (n, pair) in pairs.iter().enumerate() {
        if report.merged >= MAX_MERGES {
            break;
        }
        let incumbent_label = if pair.incumbent_same { RelationType::Same } else { RelationType::Unrelated };
        let mut s1: Option<S1Answer> = None;
        if n < MAX_SHADOW {
            let state = format!("memory A:\n{}\n\nmemory B:\n{}", pair.keep.fact, pair.drop.fact);
            let t0 = Instant::now();
            s1 = client.decide(BANK_RELATION, "CHOICE", NODE, &run_id, CONSOLIDATE_INSTRUCTIONS, &state, &labels).await;
            if s1.is_some() {
                report.shadow_decisions += 1;
                ledger::record(ledger::s1_decision_row(ctx.clone(), &client.backend, s1_live, t0.elapsed().as_millis() as u64, None));
            }
        }
        let s1_same = s1.as_ref().map(|a| a.answer == RelationType::Same.as_str() && a.confidence >= S1_LIVE_MIN_CONFIDENCE);
        // Live decision: S1 once its bank is live (incumbent when S1 is down).
        let (merge, origin) = match (s1_live, s1_same) {
            (true, Some(same)) => (same, "s1"),
            _ => (pair.incumbent_same, rel::ORIGIN_INCUMBENT),
        };
        let merged = if merge {
            let conn = db.connect().map_err(sql_err)?;
            apply_merge(&conn, user_id, pair, origin, s1.as_ref().map(|a| a.decision_id.as_str())).map_err(sql_err)?
        } else {
            false
        };
        if merged {
            report.merged += 1;
        }
        if let Some(ans) = &s1 {
            if let Ok(conn) = db.connect() {
                let _ = rel::record_decision(&conn, user_id, BANK_RELATION, ans, &run_id, Some(&pair.drop.id), Some(&pair.keep.id), Some(incumbent_label.as_str()));
            }
            // Truth for calibration: the incumbent's label while it decides.
            if !s1_live && client.reporter.report(&ans.decision_id, incumbent_label.as_str(), "memory.consolidation.incumbent").await {
                report.outcomes_reported += 1;
            }
        }
    }

    let conn = db.connect().map_err(sql_err)?;
    report.decayed = decay(&conn, user_id).map_err(sql_err)?;
    conn.execute(
        "UPDATE memory_consolidation_runs SET finished_at = CURRENT_TIMESTAMP, scanned = ?2, merged = ?3,
                shadow_decisions = ?4, decayed = ?5 WHERE id = ?1",
        params![run_id, report.scanned as i64, report.merged as i64, report.shadow_decisions as i64, report.decayed as i64],
    )
    .map_err(sql_err)?;
    let mut job_ctx = ctx;
    job_ctx.tier = Some("S0".into());
    job_ctx.lane = Some("local".into());
    ledger::record(LedgerRow {
        source: "internal",
        ctx: job_ctx,
        provider_id: Some("allternit".into()),
        model_id: Some(NODE.into()),
        latency_ms: started.elapsed().as_millis() as i64,
        status: "ok".into(),
        idempotency_key: Some(format!("memory-consolidation:{run_id}")),
        ..Default::default()
    });
    Ok(report)
}

/// Users with valid facts whose last run is older than [`RUN_EVERY`].
pub fn due_users(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<String>> {
    let secs = RUN_EVERY.as_secs() as i64;
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT f.user_id FROM memory_facts f
         WHERE f.valid_until IS NULL AND NOT EXISTS (
             SELECT 1 FROM memory_consolidation_runs r
             WHERE r.user_id = f.user_id AND r.started_at > datetime('now', '-{secs} seconds'))
         LIMIT ?1"
    ))?;
    let rows = stmt.query_map(params![limit as i64], |r| r.get(0))?;
    rows.collect()
}

/// Background schedule: every 6 h, consolidate up to 50 due users.
/// `ALLTERNIT_MEMORY_CONSOLIDATION=0` disables.
pub fn spawn(state: Arc<AppState>) {
    if std::env::var("ALLTERNIT_MEMORY_CONSOLIDATION").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    tokio::spawn(async move {
        // Let boot settle before the first pass.
        tokio::time::sleep(Duration::from_secs(300)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(6 * 3600));
        loop {
            tick.tick().await;
            let db = state.db.clone();
            let users = tokio::task::spawn_blocking(move || db.connect().and_then(|c| due_users(&c, 50))).await;
            let Ok(Ok(users)) = users else { continue };
            let client = S1Client::from_env();
            let live = relation_bank_live();
            for user in users {
                match run_for_user(&state.db, &client, &user, live).await {
                    Ok(r) => tracing::info!(user = %user, merged = r.merged, decayed = r.decayed, shadow = r.shadow_decisions, "memory consolidation"),
                    Err(e) => tracing::warn!(user = %user, error = %e, "memory consolidation failed"),
                }
            }
        }
    });
}

// ─── Store adapters ─────────────────────────────────────────────────────────

/// Sources allowed to write through the adapter routes.
pub const ADAPTER_SOURCES: &[&str] = &["gizzi.memdir", "gizzi.brain", "memory-agent", "notes"];

#[derive(Debug, Deserialize)]
pub struct AdapterItem {
    pub external_id: String,
    pub text: String,
    #[serde(default)]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct UpsertReport {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub fact_ids: Vec<String>,
}

fn content_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.trim().as_bytes()))
}

/// Upsert external items as canonical facts. A changed item writes a new
/// fact that `updates` the old one (soft supersession, history kept).
pub fn adapter_upsert(conn: &Connection, user_id: &str, source: &str, items: &[AdapterItem]) -> rusqlite::Result<UpsertReport> {
    let mut report = UpsertReport::default();
    for item in items {
        let text = item.text.trim();
        if text.is_empty() || item.external_id.is_empty() {
            continue;
        }
        let hash = content_hash(text);
        let mtype = item.memory_type.as_deref().and_then(MemoryType::parse).map(|t| t.as_str());
        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT l.fact_id, l.content_hash FROM memory_adapter_links l JOIN memory_facts f ON f.id = l.fact_id
                 WHERE l.user_id = ?1 AND l.source = ?2 AND l.external_id = ?3 AND f.valid_until IS NULL",
                params![user_id, source, item.external_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((fact_id, h)) = &existing {
            if *h == hash {
                report.unchanged += 1;
                report.fact_ids.push(fact_id.clone());
                continue;
            }
        }
        let fact_id = format!("fact_{}", Uuid::new_v4().simple());
        conn.execute(
            "INSERT INTO memory_facts (id, user_id, agent_id, fact, confidence, memory_type) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![fact_id, user_id, item.agent_id, text, item.confidence.unwrap_or(0.9).clamp(0.0, 1.0), mtype],
        )?;
        if let Some((old, _)) = &existing {
            rel::write_relation(conn, user_id, (NodeKind::Fact, &fact_id), RelationType::Updates, (NodeKind::Fact, old), 1.0, rel::ORIGIN_USER, None)?;
            report.updated += 1;
        } else {
            report.created += 1;
        }
        conn.execute(
            "INSERT INTO memory_adapter_links (user_id, source, external_id, fact_id, content_hash) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_id, source, external_id) DO UPDATE SET fact_id = excluded.fact_id,
               content_hash = excluded.content_hash, updated_at = CURRENT_TIMESTAMP",
            params![user_id, source, item.external_id, fact_id, hash],
        )?;
        report.fact_ids.push(fact_id);
    }
    Ok(report)
}

/// The external store deleted these items: retire their facts (soft).
pub fn adapter_delete(conn: &Connection, user_id: &str, source: &str, external_ids: &[String]) -> rusqlite::Result<usize> {
    let mut n = 0;
    for ext in external_ids {
        let fact: Option<String> = conn
            .query_row(
                "SELECT fact_id FROM memory_adapter_links WHERE user_id = ?1 AND source = ?2 AND external_id = ?3",
                params![user_id, source, ext],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(f) = fact {
            if rel::supersede_fact(conn, user_id, &f)? {
                n += 1;
            }
            conn.execute(
                "DELETE FROM memory_adapter_links WHERE user_id = ?1 AND source = ?2 AND external_id = ?3",
                params![user_id, source, ext],
            )?;
        }
    }
    Ok(n)
}

// ─── Routes ─────────────────────────────────────────────────────────────────

pub fn memory_consolidation_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memory/consolidation/run", post(run_now))
        .route("/memory/consolidation/status", get(status))
        .route("/memory/adapters/upsert", post(upsert_route))
        .route("/memory/adapters/delete", post(delete_route))
        .route("/memory/adapters/search", post(search_route))
}

fn bad(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": "memory_consolidation_error", "message": msg.into() }))).into_response()
}

async fn run_now(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    match run_for_user(&state.db, &S1Client::from_env(), &user.user_id, relation_bank_live()).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn status(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<serde_json::Value>> {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, started_at, finished_at, scanned, merged, shadow_decisions, decayed, live_bank
             FROM memory_consolidation_runs WHERE user_id = ?1 ORDER BY started_at DESC LIMIT 10",
        )?;
        let rows = stmt.query_map(params![user.user_id], |r| {
            Ok(json!({ "id": r.get::<_, String>(0)?, "started_at": r.get::<_, String>(1)?, "finished_at": r.get::<_, Option<String>>(2)?,
                "scanned": r.get::<_, i64>(3)?, "merged": r.get::<_, i64>(4)?, "shadow_decisions": r.get::<_, i64>(5)?,
                "decayed": r.get::<_, i64>(6)?, "live_bank": r.get::<_, String>(7)? }))
        })?;
        rows.collect()
    })
    .await;
    match res {
        Ok(Ok(runs)) => Json(json!({ "runs": runs, "s1_relation_live": relation_bank_live() })).into_response(),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpsertBody {
    pub source: String,
    pub items: Vec<AdapterItem>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteBody {
    pub source: String,
    pub external_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchBody {
    pub query: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

fn check_source(source: &str) -> Result<(), Response> {
    if ADAPTER_SOURCES.contains(&source) {
        Ok(())
    } else {
        Err(bad(StatusCode::BAD_REQUEST, format!("unknown source; one of {}", ADAPTER_SOURCES.join(", "))))
    }
}

async fn upsert_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<UpsertBody>) -> Response {
    if let Err(r) = check_source(&body.source) {
        return r;
    }
    if body.items.len() > 500 {
        return bad(StatusCode::BAD_REQUEST, "at most 500 items per call");
    }
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<UpsertReport> {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        let r = adapter_upsert(&tx, &user.user_id, &body.source, &body.items)?;
        tx.commit()?;
        Ok(r)
    })
    .await;
    match res {
        Ok(Ok(r)) => Json(r).into_response(),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<DeleteBody>) -> Response {
    if let Err(r) = check_source(&body.source) {
        return r;
    }
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || db.connect().and_then(|c| adapter_delete(&c, &user.user_id, &body.source, &body.external_ids))).await;
    match res {
        Ok(Ok(n)) => Json(json!({ "retired": n })).into_response(),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Read path for adapters: the canonical hybrid recall over the user's
/// memory, with each hit's adapter link (source, external id) when it has one.
async fn search_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<SearchBody>) -> Response {
    let limit = body.limit.unwrap_or(10).clamp(1, 50);
    let qv = crate::memory_index::global().embed_or_hash(&[body.query.clone()], crate::memory_index::InputType::Query).await;
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        let hits = crate::memory_kernel_service::recall_with_embedding(&db, &user.user_id, body.agent_id.as_deref(), None, &body.query, Some(&qv), limit)
            .map_err(|e| e.to_string())?;
        let conn = db.connect().map_err(sql_err)?;
        let mut links: HashMap<String, (String, String)> = HashMap::new();
        for h in hits.iter().filter(|h| h.item_type == "fact") {
            if let Some(l) = conn
                .query_row("SELECT source, external_id FROM memory_adapter_links WHERE fact_id = ?1 AND user_id = ?2", params![h.id, user.user_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .optional()
                .map_err(sql_err)?
            {
                links.insert(h.id.clone(), l);
            }
        }
        Ok(hits
            .into_iter()
            .map(|h| {
                let link = links.get(&h.id);
                json!({ "id": h.id, "type": h.item_type, "text": h.content, "score": h.score,
                    "source": link.map(|l| l.0.clone()), "external_id": link.map(|l| l.1.clone()) })
            })
            .collect())
    })
    .await;
    match res {
        Ok(Ok(items)) => Json(json!({ "items": items })).into_response(),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests;
