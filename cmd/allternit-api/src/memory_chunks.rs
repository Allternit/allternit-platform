//! Document-chunk adapters: one memory plane for other stores' passages (C4).
//!
//! The memory agent's memories already land as canonical facts through
//! `/api/v1/memory/adapters/{upsert,delete,search}` (`memory_consolidation`).
//! Passages from document stores (open-notebook's notebook chunks) are not
//! facts: they are written here as memory documents of the caller, chunked
//! into the shared index (`memory_index`: FTS5 + EmbeddingGemma 2 vectors)
//! and embedded right away, so the memory kernel, `local_brain` and the
//! Agency context step (`agency_api::memory_context`, documents space) all
//! recall them.
//!
//! Routes (all under `/api/v1`, authenticated; the scope is always the
//! authenticated user, never a field of the request):
//! - `POST /memory/adapters/chunks/upsert` `{source, collection, items:
//!   [{external_id, text, title?}]}`: idempotent by (user, source,
//!   collection, external id); unchanged items are no-ops, changed ones are
//!   re-chunked and re-embedded.
//! - `POST /memory/adapters/chunks/delete` `{source, collection,
//!   external_ids?}`: removes the listed items, or the whole collection.
//! - `POST /memory/adapters/chunks/search` `{query, source?, collection?,
//!   limit?}`: hybrid search over the caller's chunks (optionally one source
//!   / collection), returning text and the external ids.
//!
//! Storage needs no migration: an item is a `memory_documents` row whose id
//! is derived from (user, source, collection, external id), so one user can
//! never address another user's row, and `source_url` carries the adapter
//! key `adapter://{source}/{collection}/{external_id}` for the reverse map.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::DbHandle;
use crate::memory_index::{self, Embedded, InputType, Scope};
use crate::{auth::AuthUser, AppState};

/// Stores allowed to write passages through these routes.
pub const CHUNK_SOURCES: &[&str] = &["open-notebook", "memory-agent"];
pub const MAX_ITEMS: usize = 256;
pub const MAX_TEXT_CHARS: usize = 32_000;
const MAX_EXTERNAL_ID: usize = 256;
const MAX_COLLECTION: usize = 128;
const KEY_PREFIX: &str = "adapter://";

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkItem {
    pub external_id: String,
    pub text: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct ChunkUpsertReport {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
    /// Index chunks written (and embedded) by this call.
    pub chunks: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChunkHit {
    pub external_id: Option<String>,
    pub source: Option<String>,
    pub collection: Option<String>,
    pub document_id: String,
    pub title: String,
    /// The item's text as it was written.
    pub text: String,
    pub score: f64,
    pub similarity: Option<f64>,
}

fn valid_name(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

pub fn check_source(source: &str) -> Result<(), String> {
    if CHUNK_SOURCES.contains(&source) {
        Ok(())
    } else {
        Err(format!("unknown source; one of {}", CHUNK_SOURCES.join(", ")))
    }
}

pub fn check_collection(collection: &str) -> Result<(), String> {
    if valid_name(collection, MAX_COLLECTION) {
        Ok(())
    } else {
        Err("collection must be 1-128 chars of [A-Za-z0-9._:-]".into())
    }
}

fn valid_external_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_EXTERNAL_ID && !id.chars().any(char::is_control)
}

/// Document id of an item. Derived from the authenticated user, so an id
/// computed for one user can never name another user's row.
pub fn document_id(user_id: &str, source: &str, collection: &str, external_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for part in [user_id, source, collection, external_id] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    format!("mdoc_ext_{}", &hex::encode(h.finalize())[..32])
}

fn adapter_key(source: &str, collection: &str, external_id: &str) -> String {
    format!("{KEY_PREFIX}{source}/{collection}/{external_id}")
}

/// (source, collection, external id) of an adapter key.
fn parse_key(key: &str) -> Option<(String, String, String)> {
    let mut parts = key.strip_prefix(KEY_PREFIX)?.splitn(3, '/');
    Some((parts.next()?.to_string(), parts.next()?.to_string(), parts.next()?.to_string()))
}

/// Upsert items into the caller's memory documents and chunk them into the
/// shared index. Returns the report and the new (chunk id, text) pairs to
/// embed. Run inside one transaction so the background indexer never sees a
/// document without its chunks.
pub fn upsert_chunks(
    conn: &Connection,
    user_id: &str,
    source: &str,
    collection: &str,
    items: &[ChunkItem],
) -> rusqlite::Result<(ChunkUpsertReport, Vec<(String, String)>)> {
    let mut report = ChunkUpsertReport::default();
    let mut to_embed = Vec::new();
    for item in items {
        let text = item.text.trim();
        if text.is_empty() || !valid_external_id(&item.external_id) || text.chars().count() > MAX_TEXT_CHARS {
            report.skipped += 1;
            continue;
        }
        let title = item.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{source} {collection}"));
        let id = document_id(user_id, source, collection, &item.external_id);
        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT title, COALESCE(content, '') FROM memory_documents WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match &existing {
            Some((t, c)) if *t == title && c == text => {
                report.unchanged += 1;
                continue;
            }
            Some(_) => {
                // The V206 update trigger drops the old chunks (and their vectors).
                conn.execute(
                    "UPDATE memory_documents SET title = ?3, content = ?4, updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2",
                    params![id, user_id, title, text],
                )?;
                report.updated += 1;
            }
            None => {
                conn.execute(
                    "INSERT INTO memory_documents (id, user_id, title, content, source_type, source_url) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, user_id, title, text, source, adapter_key(source, collection, &item.external_id)],
                )?;
                report.created += 1;
            }
        }
        // Same text shape as `chunk_pending_documents`, so keyword search matches titles too.
        let chunks = memory_index::insert_chunks_with_ids(conn, user_id, "document", &id, &format!("{title}\n\n{text}"))?;
        conn.execute(
            "UPDATE memory_documents SET is_indexed = 1, chunk_count = ?2 WHERE id = ?1",
            params![id, chunks.len() as i64],
        )?;
        report.chunks += chunks.len();
        to_embed.extend(chunks);
    }
    Ok((report, to_embed))
}

/// Write embeddings for freshly written chunks (scope = the user).
pub fn store_embeddings(conn: &Connection, user_id: &str, chunks: &[(String, String)], emb: &Embedded) -> rusqlite::Result<()> {
    for ((id, _), v) in chunks.iter().zip(emb.vectors.iter()) {
        memory_index::upsert_embedding(conn, user_id, "chunk", id, &emb.model, v)?;
    }
    Ok(())
}

/// Delete the listed items of a collection, or the whole collection when
/// `external_ids` is None. Only the caller's rows are touched.
pub fn delete_chunks(
    conn: &Connection,
    user_id: &str,
    source: &str,
    collection: &str,
    external_ids: Option<&[String]>,
) -> rusqlite::Result<usize> {
    match external_ids {
        Some(ids) => {
            let mut n = 0;
            for ext in ids {
                n += conn.execute(
                    "DELETE FROM memory_documents WHERE id = ?1 AND user_id = ?2",
                    params![document_id(user_id, source, collection, ext), user_id],
                )?;
            }
            Ok(n)
        }
        None => {
            let prefix = format!("{KEY_PREFIX}{source}/{collection}/");
            conn.execute(
                "DELETE FROM memory_documents WHERE user_id = ?1 AND source_type = ?2
                   AND substr(source_url, 1, length(?3)) = ?3",
                params![user_id, source, prefix],
            )
        }
    }
}

/// Hybrid (FTS5 + vector, RRF) search over the caller's document chunks,
/// optionally only one source / collection. One hit per item, best first.
pub fn search_chunks(
    conn: &Connection,
    user_id: &str,
    source: Option<&str>,
    collection: Option<&str>,
    query: &str,
    qv: Option<&Embedded>,
    limit: usize,
) -> rusqlite::Result<Vec<ChunkHit>> {
    // Restrict to one source / collection through the chunk ids it owns.
    let only: Option<HashSet<String>> = match source {
        None => None,
        Some(src) => {
            let prefix = match collection {
                Some(c) => format!("{KEY_PREFIX}{src}/{c}/"),
                None => format!("{KEY_PREFIX}{src}/"),
            };
            let mut stmt = conn.prepare(
                "SELECT c.id FROM memory_index_chunks c JOIN memory_documents d ON d.id = c.source_id
                 WHERE c.scope = ?1 AND c.source_type = 'document' AND d.user_id = ?1 AND d.source_type = ?2
                   AND substr(d.source_url, 1, length(?3)) = ?3",
            )?;
            let ids = stmt.query_map(params![user_id, src, prefix], |r| r.get::<_, String>(0))?.collect::<Result<HashSet<_>, _>>()?;
            if ids.is_empty() {
                return Ok(vec![]);
            }
            Some(ids)
        }
    };
    let types = ["chunk"];
    let scope = Scope { scope: user_id, target_types: &types, only_ids: only.as_ref() };
    let v = qv.and_then(|e| e.vectors.first().map(|v| (e.model.as_str(), v.as_slice())));
    let hits = memory_index::hybrid_search(conn, &scope, query, v, limit * 3)?;
    if hits.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<&str> = hits.iter().map(|h| h.target_id.as_str()).collect();
    let placeholders = (2..ids.len() + 2).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT c.id, d.id, d.title, COALESCE(d.content, ''), d.source_url FROM memory_index_chunks c
         JOIN memory_documents d ON d.id = c.source_id AND c.source_type = 'document'
         WHERE c.scope = ?1 AND d.user_id = ?1 AND c.id IN ({placeholders})"
    ))?;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&user_id];
    for id in &ids {
        args.push(id);
    }
    let rows: HashMap<String, (String, String, String, Option<String>)> = stmt
        .query_map(params_from_iter(args), |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))))?
        .collect::<Result<_, _>>()?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for h in hits {
        let Some((doc_id, title, content, url)) = rows.get(&h.target_id) else { continue };
        if !seen.insert(doc_id.clone()) {
            continue;
        }
        let key = url.as_deref().and_then(parse_key);
        out.push(ChunkHit {
            external_id: key.as_ref().map(|k| k.2.clone()),
            source: key.as_ref().map(|k| k.0.clone()),
            collection: key.as_ref().map(|k| k.1.clone()),
            document_id: doc_id.clone(),
            title: title.clone(),
            text: content.clone(),
            score: h.score,
            similarity: h.similarity,
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// Upsert + embed: the route's work, callable without HTTP.
pub async fn ingest(db: &DbHandle, user_id: &str, source: &str, collection: &str, items: Vec<ChunkItem>) -> Result<ChunkUpsertReport, String> {
    let (db2, uid, src, col) = (db.clone(), user_id.to_string(), source.to_string(), collection.to_string());
    let (report, chunks) = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let mut conn = db2.connect()?;
        let tx = conn.transaction()?;
        let r = upsert_chunks(&tx, &uid, &src, &col, &items)?;
        tx.commit()?;
        Ok(r)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    if !chunks.is_empty() {
        // Embed now so the items are searchable by vector at once; with the
        // endpoint down this writes hash vectors, which the background
        // indexer replaces with the real model's once it is back.
        let texts: Vec<String> = chunks.iter().map(|c| c.1.clone()).collect();
        let emb = memory_index::global().embed_or_hash(&texts, InputType::Document).await;
        let (db2, uid) = (db.clone(), user_id.to_string());
        tokio::task::spawn_blocking(move || db2.connect().and_then(|c| store_embeddings(&c, &uid, &chunks, &emb)))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    }
    Ok(report)
}

// ─── Routes ─────────────────────────────────────────────────────────────────

pub fn memory_chunks_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memory/adapters/chunks/upsert", post(upsert_route))
        .route("/memory/adapters/chunks/delete", post(delete_route))
        .route("/memory/adapters/chunks/search", post(search_route))
}

fn bad(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": "memory_chunks_error", "message": msg.into() }))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct UpsertBody {
    pub source: String,
    pub collection: String,
    pub items: Vec<ChunkItem>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteBody {
    pub source: String,
    pub collection: String,
    #[serde(default)]
    pub external_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct SearchBody {
    pub query: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

async fn upsert_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<UpsertBody>) -> Response {
    if let Err(e) = check_source(&body.source).and_then(|_| check_collection(&body.collection)) {
        return bad(StatusCode::BAD_REQUEST, e);
    }
    if body.items.len() > MAX_ITEMS {
        return bad(StatusCode::BAD_REQUEST, format!("at most {MAX_ITEMS} items per call"));
    }
    match ingest(&state.db, &user.user_id, &body.source, &body.collection, body.items).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn delete_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<DeleteBody>) -> Response {
    if let Err(e) = check_source(&body.source).and_then(|_| check_collection(&body.collection)) {
        return bad(StatusCode::BAD_REQUEST, e);
    }
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        db.connect().and_then(|c| delete_chunks(&c, &user.user_id, &body.source, &body.collection, body.external_ids.as_deref()))
    })
    .await;
    match res {
        Ok(Ok(n)) => Json(json!({ "deleted": n })).into_response(),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn search_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<SearchBody>) -> Response {
    if let Some(src) = &body.source {
        if let Err(e) = check_source(src) {
            return bad(StatusCode::BAD_REQUEST, e);
        }
    }
    if let Some(c) = &body.collection {
        if body.source.is_none() {
            return bad(StatusCode::BAD_REQUEST, "collection needs a source");
        }
        if let Err(e) = check_collection(c) {
            return bad(StatusCode::BAD_REQUEST, e);
        }
    }
    let limit = body.limit.unwrap_or(10).clamp(1, 50);
    let qv = memory_index::global().embed_or_hash(&[body.query.clone()], InputType::Query).await;
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        db.connect().and_then(|c| search_chunks(&c, &user.user_id, body.source.as_deref(), body.collection.as_deref(), &body.query, Some(&qv), limit))
    })
    .await;
    match res {
        Ok(Ok(items)) => Json(json!({ "items": items })).into_response(),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_db() -> DbHandle {
        std::env::set_var("ALLTERNIT_EMBED_URL", "off"); // keyword + hash vectors, no sidecar needed
        DbHandle::new(std::env::temp_dir().join(format!("allternit-mem-chunks-{}.db", uuid::Uuid::new_v4()))).unwrap()
    }

    fn item(id: &str, text: &str) -> ChunkItem {
        ChunkItem { external_id: id.into(), text: text.into(), title: Some("Field notes".into()) }
    }

    #[tokio::test]
    async fn ingest_is_idempotent_and_updates_in_place() {
        let db = test_db();
        let r = ingest(&db, "alice", "open-notebook", "nb1", vec![item("c1", "The heron nests by the north lake."), item("c2", "Otters hunt at dusk.")])
            .await
            .unwrap();
        assert_eq!((r.created, r.updated, r.unchanged, r.chunks), (2, 0, 0, 2));
        let again = ingest(&db, "alice", "open-notebook", "nb1", vec![item("c1", "The heron nests by the north lake.")]).await.unwrap();
        assert_eq!((again.created, again.unchanged, again.chunks), (0, 1, 0));
        let changed = ingest(&db, "alice", "open-notebook", "nb1", vec![item("c1", "The heron moved to the south marsh.")]).await.unwrap();
        assert_eq!((changed.updated, changed.chunks), (1, 1));

        let conn = db.connect().unwrap();
        let (docs, chunks, vectors): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM memory_documents WHERE user_id = 'alice'),
                        (SELECT COUNT(*) FROM memory_index_chunks WHERE scope = 'alice'),
                        (SELECT COUNT(*) FROM memory_embeddings WHERE user_id = 'alice' AND target_type = 'chunk')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((docs, chunks, vectors), (2, 2, 2), "the old chunk and its vector are replaced, not kept");
        let hits = search_chunks(&conn, "alice", Some("open-notebook"), Some("nb1"), "where is the heron", None, 5).unwrap();
        assert_eq!(hits[0].external_id.as_deref(), Some("c1"));
        assert_eq!(hits[0].text, "The heron moved to the south marsh.");
        assert_eq!(hits[0].collection.as_deref(), Some("nb1"));
    }

    #[tokio::test]
    async fn chunks_stay_in_their_owners_scope_and_reach_agency_recall() {
        let db = test_db();
        ingest(&db, "alice", "open-notebook", "nb1", vec![item("c1", "Quarterly budget review moves to Thursday.")]).await.unwrap();
        // Bob uses the same collection and external id: a different row.
        ingest(&db, "bob", "open-notebook", "nb1", vec![item("c1", "Bob's quarterly budget lives in the finance sheet.")]).await.unwrap();
        let conn = db.connect().unwrap();
        let alice = search_chunks(&conn, "alice", None, None, "quarterly budget", None, 10).unwrap();
        assert_eq!(alice.len(), 1);
        assert!(alice[0].text.contains("Thursday"));
        let bob = search_chunks(&conn, "bob", Some("open-notebook"), Some("nb1"), "quarterly budget", None, 10).unwrap();
        assert_eq!(bob.len(), 1);
        assert!(bob[0].text.contains("finance sheet"));
        assert_ne!(document_id("alice", "open-notebook", "nb1", "c1"), document_id("bob", "open-notebook", "nb1", "c1"));
        // Bob cannot delete Alice's item by naming it.
        assert_eq!(delete_chunks(&conn, "bob", "open-notebook", "nb1", Some(&["c1".to_string()])).unwrap(), 1);
        assert_eq!(search_chunks(&conn, "alice", None, None, "quarterly budget", None, 10).unwrap().len(), 1);
        drop(conn);

        let recall = crate::agency_api::memory_context::recall(&db, "alice", "when is the quarterly budget review").await.unwrap();
        assert!(recall.text.contains("Thursday"), "{}", recall.text);
        assert!(!recall.text.contains("finance sheet"), "{}", recall.text);
    }

    #[tokio::test]
    async fn collection_filter_and_whole_collection_delete() {
        let db = test_db();
        ingest(&db, "alice", "open-notebook", "nb1", vec![item("a", "Lighthouse keepers log the weather.")]).await.unwrap();
        ingest(&db, "alice", "open-notebook", "nb10", vec![item("a", "Lighthouse lamps were once whale oil.")]).await.unwrap();
        let conn = db.connect().unwrap();
        let nb1 = search_chunks(&conn, "alice", Some("open-notebook"), Some("nb1"), "lighthouse", None, 10).unwrap();
        assert_eq!(nb1.len(), 1, "nb1 must not match nb10 by prefix");
        assert_eq!(delete_chunks(&conn, "alice", "open-notebook", "nb1", None).unwrap(), 1);
        assert!(search_chunks(&conn, "alice", Some("open-notebook"), Some("nb1"), "lighthouse", None, 10).unwrap().is_empty());
        assert_eq!(search_chunks(&conn, "alice", Some("open-notebook"), None, "lighthouse", None, 10).unwrap().len(), 1);
        let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM memory_index_chunks WHERE scope = 'alice'", [], |r| r.get(0)).unwrap();
        assert_eq!(chunks, 1, "deleting a document drops its chunks");
    }

    #[tokio::test]
    async fn routes_scope_to_the_authenticated_user_and_validate() {
        let dir = std::env::temp_dir().join(format!("allternit-mem-chunks-routes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("ALLTERNIT_EMBED_URL", "off");
        let state = crate::test_helpers::app_state(&dir).await;
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
        let app = |u: AuthUser| memory_chunks_router().layer(axum::Extension(u)).with_state(state.clone());
        let call = |u: AuthUser, path: &'static str, body: serde_json::Value| {
            let app = app(u);
            async move {
                let resp = app
                    .oneshot(Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
                    .await
                    .unwrap();
                let status = resp.status();
                let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
                (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
            }
        };
        // A user id in the body is ignored: the scope is the authenticated user.
        let (s, v) = call(
            user("alice"),
            "/memory/adapters/chunks/upsert",
            json!({"source": "open-notebook", "collection": "nb1", "user_id": "bob", "items": [{"external_id": "c1", "text": "Seed the tomatoes in March."}]}),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["created"], 1);
        let (_, v) = call(user("bob"), "/memory/adapters/chunks/search", json!({"query": "tomatoes"})).await;
        assert_eq!(v["items"].as_array().unwrap().len(), 0);
        let (_, v) = call(user("alice"), "/memory/adapters/chunks/search", json!({"query": "tomatoes", "source": "open-notebook", "collection": "nb1"})).await;
        assert_eq!(v["items"][0]["external_id"], "c1");
        assert_eq!(v["items"][0]["text"], "Seed the tomatoes in March.");

        let (s, _) = call(user("alice"), "/memory/adapters/chunks/upsert", json!({"source": "elsewhere", "collection": "nb1", "items": []})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(user("alice"), "/memory/adapters/chunks/upsert", json!({"source": "open-notebook", "collection": "../x", "items": []})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(user("alice"), "/memory/adapters/chunks/search", json!({"query": "x", "collection": "nb1"})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, v) = call(user("alice"), "/memory/adapters/chunks/delete", json!({"source": "open-notebook", "collection": "nb1"})).await;
        assert_eq!((s, v["deleted"].as_u64()), (StatusCode::OK, Some(1)));
    }
}
