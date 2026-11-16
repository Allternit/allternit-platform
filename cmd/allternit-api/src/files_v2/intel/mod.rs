//! Files intelligence (plan step 16): full text index, meaning index, tags,
//! collections, duplicate report, provenance labels, thumbnails and previews,
//! and the explicit summarize action.
//!
//! - **Where it lives.** Everything is kept in `intel.sqlite` beside the engine
//!   DB, on the computer that owns the files. Nothing is central.
//! - **D12.** Indexing, embeddings and previews run on one background thread
//!   inside the runtime process, so they only run while the computer is
//!   already awake. They never wake it and never touch activity: no code in
//!   this module calls an idle reset or a wake path
//!   (`tests::intel_never_wakes_or_touches_activity` guards it), and the
//!   embedding call goes to the local endpoint, not to company compute.
//! - **CPU.** See `budget`: 25% of one core by default, one thread, 8 files
//!   per pass, doubling back-off when idle. On Linux the thread is also niced.
//! - **Protected roots** (Memory Drive) are not indexed unless the owner opts
//!   a root in with `PUT /intel/roots/:root_id`. Opting out forgets the rows.
//! - **Not automatic.** Summaries and transcripts are explicit actions or a
//!   folder rule the owner created. Transcription has no local model on this
//!   runtime, so it answers `unsupported`.
//! - **Text only.** Plain text and code are indexed (a file that is UTF-8 with


pub mod budget;
pub mod embed;
pub(crate) mod routes;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use allternit_files_core::{Item, ItemKind, PrincipalKind, Provenance, SearchHit, SearchMode, SearchRequest, SearchResponse, SearchResponseIndexState};
use allternit_files_engine::RootRecord;
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension};

use self::budget::{Backoff, Budget, MAX_TEXT_BYTES};
use self::embed::{normalized, Embedder, LocalEndpointEmbedder, NullEmbedder};
use super::{ApiError, Fail, Inner, Need, Req};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS indexed(
  item_id TEXT PRIMARY KEY, root_id TEXT NOT NULL, content_hash TEXT NOT NULL, modified_at TEXT NOT NULL,
  indexed_at TEXT NOT NULL, has_text INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS indexed_root ON indexed(root_id);
CREATE VIRTUAL TABLE IF NOT EXISTS docs USING fts5(item_id UNINDEXED, name, body, tokenize = 'unicode61 remove_diacritics 2');
CREATE TABLE IF NOT EXISTS vectors(item_id TEXT PRIMARY KEY, model TEXT NOT NULL, dim INTEGER NOT NULL, vec BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS previews(
  item_id TEXT NOT NULL, kind TEXT NOT NULL, mime TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(item_id, kind));
CREATE TABLE IF NOT EXISTS summaries(item_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL, body TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tags(item_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY(item_id, tag));
CREATE INDEX IF NOT EXISTS tags_tag ON tags(tag);
CREATE TABLE IF NOT EXISTS provenance(item_id TEXT PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS collections(collection_id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS collection_items(
  collection_id TEXT NOT NULL, item_id TEXT NOT NULL, added_at TEXT NOT NULL, PRIMARY KEY(collection_id, item_id));
CREATE TABLE IF NOT EXISTS root_opt_in(root_id TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS folder_rules(rule_id TEXT PRIMARY KEY, folder_id TEXT NOT NULL, action TEXT NOT NULL, created_at TEXT NOT NULL);
"#;

const PREVIEW_CHARS: usize = 2000;
const EMBED_TEXT_CHARS: usize = 4000;
const THUMB_PX: u32 = 256;
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 50_000_000;
const MAX_TAGS_PER_ITEM: usize = 50;
const CANDIDATE_PAGE: usize = 500;
const FORGET_BATCH: usize = 200;

/// A file the engine index has that this index doesn't hold at its current hash.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub item_id: String,
    pub root_id: String,
    pub name: String,
    pub content_hash: String,
    pub modified_at: String,
}

pub struct TextHit {
    pub item_id: String,
    pub score: f64,
    pub snippet: Option<String>,
}

pub struct DedupGroup {
    pub content_hash: String,
    pub size: u64,
    pub item_ids: Vec<String>,
}

pub struct Intel {
    conn: Mutex<Connection>,
    embedder: Mutex<Arc<dyn Embedder>>,
    budget: Mutex<Budget>,
    caught_up: AtomicBool,
    forget_cursor: AtomicUsize,
}

impl Intel {
    pub fn open(path: &Path, embedder: Arc<dyn Embedder>, budget: Budget) -> rusqlite::Result<Self> {
        let c = Connection::open(path)?;
        c.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        c.busy_timeout(Duration::from_secs(10))?;
        c.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(c),
            embedder: Mutex::new(embedder),
            budget: Mutex::new(budget),
            caught_up: AtomicBool::new(false),
            forget_cursor: AtomicUsize::new(0),
        })
    }

    /// The provider for this process: the local endpoint when a tokio runtime
    /// is running, else none.
    pub fn default_embedder() -> Arc<dyn Embedder> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => Arc::new(LocalEndpointEmbedder::new(handle)),
            Err(_) => Arc::new(NullEmbedder),
        }
    }

    pub fn embedder(&self) -> Arc<dyn Embedder> {
        self.embedder.lock().expect("intel embedder lock").clone()
    }

    pub fn set_embedder(&self, e: Arc<dyn Embedder>) {
        *self.embedder.lock().expect("intel embedder lock") = e;
    }

    pub fn budget(&self) -> Budget {
        *self.budget.lock().expect("intel budget lock")
    }

    pub fn set_budget(&self, b: Budget) {
        *self.budget.lock().expect("intel budget lock") = b;
    }

    pub fn caught_up(&self) -> bool {
        self.caught_up.load(Ordering::Relaxed)
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("intel lock")
    }

    // ---- wire decoration ----

    /// Adds tags, provenance and the thumbnail flag to an item on its way out.
    pub fn decorate(&self, item: &mut Item) {
        let c = self.c();
        let tags = strings(&c, "SELECT tag FROM tags WHERE item_id = ?1 ORDER BY tag", params![item.id]).unwrap_or_default();
        if !tags.is_empty() {
            item.tags = Some(tags);
        }
        let body: Option<String> =
            c.query_row("SELECT body FROM provenance WHERE item_id = ?1", params![item.id], |r| r.get(0)).optional().unwrap_or(None);
        if let Some(p) = body.and_then(|b| serde_json::from_str::<Provenance>(&b).ok()) {
            item.provenance = Some(p);
        }
        let thumb: Option<i64> = c
            .query_row("SELECT 1 FROM previews WHERE item_id = ?1 AND kind = 'thumbnail'", params![item.id], |r| r.get(0))
            .optional()
            .unwrap_or(None);
        if thumb.is_some() {
            item.has_thumbnail = Some(true);
        }
    }

    // ---- tags, provenance ----

    pub fn tags_of(&self, item_id: &str) -> rusqlite::Result<Vec<String>> {
        strings(&self.c(), "SELECT tag FROM tags WHERE item_id = ?1 ORDER BY tag", params![item_id])
    }

    /// Adds then removes; returns the item's tags.
    pub fn change_tags(&self, item_id: &str, add: &[String], remove: &[String]) -> Result<Vec<String>, ApiError> {
        let c = self.c();
        for t in add {
            c.execute("INSERT OR IGNORE INTO tags(item_id, tag) VALUES(?1, ?2)", params![item_id, t]).map_err(|_| ApiError::internal())?;
        }
        for t in remove {
            c.execute("DELETE FROM tags WHERE item_id = ?1 AND tag = ?2", params![item_id, t]).map_err(|_| ApiError::internal())?;
        }
        let tags = strings(&c, "SELECT tag FROM tags WHERE item_id = ?1 ORDER BY tag", params![item_id]).map_err(|_| ApiError::internal())?;
        if tags.len() > MAX_TAGS_PER_ITEM {
            // Undo this call's additions so the limit holds.
            for t in add {
                let _ = c.execute("DELETE FROM tags WHERE item_id = ?1 AND tag = ?2", params![item_id, t]);
            }
            return Err(ApiError::invalid(format!("An item can have at most {MAX_TAGS_PER_ITEM} tags.")));
        }
        Ok(tags)
    }

    pub fn items_with_tag(&self, tag: &str) -> rusqlite::Result<Vec<String>> {
        strings(&self.c(), "SELECT item_id FROM tags WHERE tag = ?1 ORDER BY item_id", params![tag])
    }

    pub fn tag_counts(&self) -> rusqlite::Result<Vec<(String, u64)>> {
        let c = self.c();
        let mut s = c.prepare("SELECT tag, COUNT(*) FROM tags GROUP BY tag ORDER BY COUNT(*) DESC, tag")?;
        let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64)))?;
        rows.collect()
    }

    pub fn set_provenance(&self, item_id: &str, p: &Provenance) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO provenance(item_id, body) VALUES(?1, ?2) ON CONFLICT(item_id) DO UPDATE SET body = excluded.body",
            params![item_id, serde_json::to_string(p).expect("provenance serializes")],
        )?;
        Ok(())
    }

    // ---- collections ----

    pub fn create_collection(&self, id: &str, name: &str, now: &str) -> rusqlite::Result<()> {
        self.c().execute("INSERT INTO collections(collection_id, name, created_at) VALUES(?1,?2,?3)", params![id, name, now])?;
        Ok(())
    }

    /// `(collection_id, name, item count)`.
    pub fn collections(&self) -> rusqlite::Result<Vec<(String, String, u64)>> {
        let c = self.c();
        let mut s = c.prepare(
            "SELECT c.collection_id, c.name, (SELECT COUNT(*) FROM collection_items i WHERE i.collection_id = c.collection_id) \
             FROM collections c ORDER BY c.created_at, c.collection_id",
        )?;
        let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?.max(0) as u64)))?;
        rows.collect()
    }

    pub fn collection_name(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.c().query_row("SELECT name FROM collections WHERE collection_id = ?1", params![id], |r| r.get(0)).optional()
    }

    pub fn collection_items(&self, id: &str) -> rusqlite::Result<Vec<String>> {
        strings(&self.c(), "SELECT item_id FROM collection_items WHERE collection_id = ?1 ORDER BY added_at, item_id", params![id])
    }

    pub fn collection_change(&self, id: &str, item_id: &str, add: bool, now: &str) -> rusqlite::Result<()> {
        let c = self.c();
        if add {
            c.execute("INSERT OR IGNORE INTO collection_items(collection_id, item_id, added_at) VALUES(?1,?2,?3)", params![id, item_id, now])?;
        } else {
            c.execute("DELETE FROM collection_items WHERE collection_id = ?1 AND item_id = ?2", params![id, item_id])?;
        }
        Ok(())
    }

    pub fn delete_collection(&self, id: &str) -> rusqlite::Result<bool> {
        let c = self.c();
        c.execute("DELETE FROM collection_items WHERE collection_id = ?1", params![id])?;
        Ok(c.execute("DELETE FROM collections WHERE collection_id = ?1", params![id])? > 0)
    }

    // ---- protected-root opt-in, folder rules ----

    pub fn opted_in(&self, root_id: &str) -> rusqlite::Result<bool> {
        Ok(self.c().query_row("SELECT 1 FROM root_opt_in WHERE root_id = ?1", params![root_id], |r| r.get::<_, i64>(0)).optional()?.is_some())
    }

    pub fn set_opt_in(&self, root_id: &str, on: bool) -> rusqlite::Result<()> {
        let c = self.c();
        if on {
            c.execute("INSERT OR IGNORE INTO root_opt_in(root_id) VALUES(?1)", params![root_id])?;
        } else {
            c.execute("DELETE FROM root_opt_in WHERE root_id = ?1", params![root_id])?;
        }
        Ok(())
    }

    /// `(rule_id, folder_id, action)`.
    pub fn rules(&self) -> rusqlite::Result<Vec<(String, String, String)>> {
        let c = self.c();
        let mut s = c.prepare("SELECT rule_id, folder_id, action FROM folder_rules ORDER BY created_at, rule_id")?;
        let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        rows.collect()
    }

    pub fn add_rule(&self, id: &str, folder_id: &str, action: &str, now: &str) -> rusqlite::Result<()> {
        self.c().execute("INSERT INTO folder_rules(rule_id, folder_id, action, created_at) VALUES(?1,?2,?3,?4)", params![id, folder_id, action, now])?;
        Ok(())
    }

    pub fn delete_rule(&self, id: &str) -> rusqlite::Result<bool> {
        Ok(self.c().execute("DELETE FROM folder_rules WHERE rule_id = ?1", params![id])? > 0)
    }

    // ---- previews, summaries ----

    /// `(mime, bytes)` of a stored preview of `kind` (`thumbnail` or `text`).
    pub fn preview(&self, item_id: &str, kind: &str) -> rusqlite::Result<Option<(String, Vec<u8>)>> {
        self.c()
            .query_row("SELECT mime, data FROM previews WHERE item_id = ?1 AND kind = ?2", params![item_id, kind], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
    }

    pub fn summary(&self, item_id: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.c().query_row("SELECT content_hash, body FROM summaries WHERE item_id = ?1", params![item_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()
    }

    pub fn store_summary(&self, item_id: &str, content_hash: &str, body: &str, now: &str) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO summaries(item_id, content_hash, body, created_at) VALUES(?1,?2,?3,?4) \
             ON CONFLICT(item_id) DO UPDATE SET content_hash = excluded.content_hash, body = excluded.body, created_at = excluded.created_at",
            params![item_id, content_hash, body, now],
        )?;
        Ok(())
    }

    // ---- indexing ----

    pub fn indexed_count(&self) -> rusqlite::Result<u64> {
        Ok(self.c().query_row("SELECT COUNT(*) FROM indexed", [], |r| r.get::<_, i64>(0))?.max(0) as u64)
    }

    pub fn is_indexed(&self, item_id: &str) -> rusqlite::Result<bool> {
        Ok(self.c().query_row("SELECT 1 FROM indexed WHERE item_id = ?1", params![item_id], |r| r.get::<_, i64>(0)).optional()?.is_some())
    }

    /// The content hash this index holds for an item.
    pub fn indexed_hash(&self, item_id: &str) -> rusqlite::Result<Option<String>> {
        self.c().query_row("SELECT content_hash FROM indexed WHERE item_id = ?1", params![item_id], |r| r.get(0)).optional()
    }

    pub fn vector_models(&self) -> rusqlite::Result<Vec<String>> {
        strings(&self.c(), "SELECT DISTINCT model FROM vectors ORDER BY model", [])
    }

    fn is_current(&self, cand: &Candidate) -> rusqlite::Result<bool> {
        let hash: Option<String> = self
            .c()
            .query_row("SELECT content_hash FROM indexed WHERE item_id = ?1", params![cand.item_id], |r| r.get(0))
            .optional()?;
        Ok(hash.as_deref() == Some(cand.content_hash.as_str()))
    }

    /// Up to `limit` live files in `root_ids` that this index doesn't hold at
    /// their current content hash.
    pub fn pending(&self, engine_dir: &Path, root_ids: &[String], limit: usize) -> rusqlite::Result<Vec<Candidate>> {
        if root_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ec = Connection::open_with_flags(engine_dir.join("index.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        ec.busy_timeout(Duration::from_secs(5))?;
        let marks = (0..root_ids.len()).map(|i| format!("?{}", i + 2)).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT item_id, root_id, name, content_hash, modified_at FROM items \
             WHERE trash_id IS NULL AND kind = 'file' AND link_escapes = 0 AND parent_id IS NOT NULL \
             AND size > 0 AND content_hash IS NOT NULL AND item_id > ?1 AND root_id IN ({marks}) \
             ORDER BY item_id LIMIT {CANDIDATE_PAGE}"
        );
        let mut out = Vec::new();
        let mut after = String::new();
        loop {
            let mut values = vec![after.clone()];
            values.extend(root_ids.iter().cloned());
            let mut s = ec.prepare_cached(&sql)?;
            let rows = s.query_map(params_from_iter(values), |r| {
                Ok(Candidate { item_id: r.get(0)?, root_id: r.get(1)?, name: r.get(2)?, content_hash: r.get(3)?, modified_at: r.get(4)? })
            })?;
            let page: Vec<Candidate> = rows.collect::<rusqlite::Result<_>>()?;
            if page.is_empty() {
                return Ok(out);
            }
            for cand in page {
                after = cand.item_id.clone();
                if !self.is_current(&cand)? {
                    out.push(cand);
                    if out.len() >= limit {
                        return Ok(out);
                    }
                }
            }
        }
    }

    /// Reads and indexes one file. Returns its text when it has any.
    pub fn index_one(&self, cand: &Candidate, path: &Path, now: &str) -> rusqlite::Result<Option<String>> {
        let text = read_document(path, &cand.name);
        let thumb = if is_image_name(&cand.name) { thumbnail(path) } else { None };
        // Embed before taking the DB lock: it can wait on the network.
        let vector = text.as_ref().and_then(|t| {
            let doc = format!("{}\n{}", cand.name, t.chars().take(EMBED_TEXT_CHARS).collect::<String>());
            let e = self.embedder().embed(&[doc], false)?;
            let model = e.model;
            e.vectors.into_iter().next().map(|v| (model, normalized(v)))
        });
        let mut c = self.c();
        let tx = c.transaction()?;
        forget_in(&tx, &cand.item_id)?;
        tx.execute(
            "INSERT INTO indexed(item_id, root_id, content_hash, modified_at, indexed_at, has_text) VALUES(?1,?2,?3,?4,?5,?6)",
            params![cand.item_id, cand.root_id, cand.content_hash, cand.modified_at, now, text.is_some() as i64],
        )?;
        if let Some(t) = &text {
            tx.execute("INSERT INTO docs(item_id, name, body) VALUES(?1,?2,?3)", params![cand.item_id, cand.name, t])?;
            let preview: String = t.chars().take(PREVIEW_CHARS).collect();
            tx.execute(
                "INSERT INTO previews(item_id, kind, mime, data) VALUES(?1,'text','text/plain; charset=utf-8',?2)",
                params![cand.item_id, preview.into_bytes()],
            )?;
        }
        if let Some((model, v)) = vector {
            tx.execute(
                "INSERT INTO vectors(item_id, model, dim, vec) VALUES(?1,?2,?3,?4)",
                params![cand.item_id, model, v.len() as i64, f32_bytes(&v)],
            )?;
        }
        if let Some(png) = thumb {
            tx.execute("INSERT INTO previews(item_id, kind, mime, data) VALUES(?1,'thumbnail','image/png',?2)", params![cand.item_id, png])?;
        }
        tx.commit()?;
        Ok(text)
    }

    /// Records a file that could not be read so a pass doesn't pick it again
    /// until its content changes.
    pub fn mark_skipped(&self, cand: &Candidate, now: &str) -> rusqlite::Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        forget_in(&tx, &cand.item_id)?;
        tx.execute(
            "INSERT INTO indexed(item_id, root_id, content_hash, modified_at, indexed_at, has_text) VALUES(?1,?2,?3,?4,?5,0)",
            params![cand.item_id, cand.root_id, cand.content_hash, cand.modified_at, now],
        )?;
        tx.commit()
    }

    /// Drops everything indexed for roots that are no longer eligible.
    pub fn forget_outside(&self, eligible: &[String]) -> rusqlite::Result<usize> {
        let held = strings(&self.c(), "SELECT DISTINCT root_id FROM indexed", [])?;
        let keep: HashSet<&String> = eligible.iter().collect();
        let mut n = 0;
        for root in held.iter().filter(|r| !keep.contains(r)) {
            let c = self.c();
            let ids = strings(&c, "SELECT item_id FROM indexed WHERE root_id = ?1", params![root])?;
            for id in &ids {
                forget_in(&c, id)?;
            }
            n += ids.len();
        }
        Ok(n)
    }

    /// Drops rows of items that are gone from the drive; a rotating window of
    /// `FORGET_BATCH` rows per call.
    pub(crate) fn forget_gone(&self, v: &Inner) -> rusqlite::Result<usize> {
        let offset = self.forget_cursor.load(Ordering::Relaxed);
        let ids = strings(&self.c(), "SELECT item_id FROM indexed ORDER BY rowid LIMIT ?1 OFFSET ?2", params![FORGET_BATCH as i64, offset as i64])?;
        self.forget_cursor.store(if ids.len() < FORGET_BATCH { 0 } else { offset + FORGET_BATCH }, Ordering::Relaxed);
        let mut n = 0;
        for id in ids.iter().filter(|id| v.drive.stat(id).is_err()) {
            forget_in(&self.c(), id)?;
            n += 1;
        }
        Ok(n)
    }

    // ---- search ----

    /// Full text hits (`match_expr` output), best first.
    pub fn search_full_text(
        &self,
        expr: &str,
        root_ids: &[String],
        modified_after: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> rusqlite::Result<Vec<TextHit>> {
        if root_ids.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (0..root_ids.len()).map(|i| format!("?{}", i + 3)).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT docs.item_id, -bm25(docs), snippet(docs, 2, '', '', '...', 12) FROM docs \
             JOIN indexed i ON i.item_id = docs.item_id \
             WHERE docs MATCH ?1 AND (?2 IS NULL OR i.modified_at > ?2) AND i.root_id IN ({marks}) \
             ORDER BY bm25(docs) LIMIT {limit} OFFSET {offset}"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![expr.to_string().into(), modified_after.map(|m| m.to_string()).into()];
        values.extend(root_ids.iter().map(|r| rusqlite::types::Value::from(r.clone())));
        let c = self.c();
        let mut s = c.prepare(&sql)?;
        let rows = s.query_map(params_from_iter(values), |r| {
            let snippet: Option<String> = r.get(2)?;
            Ok(TextHit { item_id: r.get(0)?, score: r.get(1)?, snippet: snippet.filter(|s| !s.trim().is_empty()) })
        })?;
        rows.collect()
    }

    /// Nearest vectors of `model` to the unit vector `query`, best first.
    pub fn search_semantic(
        &self,
        query: &[f32],
        model: &str,
        root_ids: &[String],
        modified_after: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> rusqlite::Result<Vec<TextHit>> {
        if root_ids.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (0..root_ids.len()).map(|i| format!("?{}", i + 3)).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT v.item_id, v.vec FROM vectors v JOIN indexed i ON i.item_id = v.item_id \
             WHERE v.model = ?1 AND (?2 IS NULL OR i.modified_at > ?2) AND i.root_id IN ({marks}) \
             ORDER BY i.modified_at DESC LIMIT {}",
            crate::memory_index::VECTOR_SCAN_CAP
        );
        let mut values: Vec<rusqlite::types::Value> = vec![model.to_string().into(), modified_after.map(|m| m.to_string()).into()];
        values.extend(root_ids.iter().map(|r| rusqlite::types::Value::from(r.clone())));
        let c = self.c();
        let mut s = c.prepare(&sql)?;
        let rows = s.query_map(params_from_iter(values), |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        let mut scored: Vec<(String, f64)> = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            let v = crate::memory_index::bytes_to_f32(&blob);
            if v.len() != query.len() {
                continue;
            }
            let score: f32 = v.iter().zip(query).map(|(a, b)| a * b).sum();
            if score > 0.0 {
                scored.push((id, score as f64));
            }
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
        Ok(scored.into_iter().skip(offset).take(limit).map(|(item_id, score)| TextHit { item_id, score, snippet: None }).collect())
    }
}

fn strings(c: &Connection, sql: &str, p: impl rusqlite::Params) -> rusqlite::Result<Vec<String>> {
    let mut s = c.prepare(sql)?;
    let rows = s.query_map(p, |r| r.get::<_, String>(0))?;
    rows.collect()
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    crate::memory_index::f32_to_bytes(v)
}

/// Removes an item's rows from the content tables (not tags, collections or provenance).
fn forget_in(c: &Connection, item_id: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM docs WHERE item_id = ?1", params![item_id])?;
    c.execute("DELETE FROM vectors WHERE item_id = ?1", params![item_id])?;
    c.execute("DELETE FROM previews WHERE item_id = ?1", params![item_id])?;
    c.execute("DELETE FROM summaries WHERE item_id = ?1", params![item_id])?;
    c.execute("DELETE FROM indexed WHERE item_id = ?1", params![item_id])?;
    Ok(())
}

/// The file's text, or None for binary or empty content. At most `cap` bytes.
pub(crate) fn read_text(path: &Path, cap: u64) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    let mut r = f.take(cap);
    r.read_to_end(&mut buf).ok()?;
    if buf.is_empty() || buf.iter().take(8192).any(|b| *b == 0) {
        return None;
    }
    match std::str::from_utf8(&buf) {
        Ok(s) => Some(s.to_string()),
        // A cut in the middle of a character at the cap is fine; bad bytes are not.
        Err(e) if e.error_len().is_none() => Some(String::from_utf8_lossy(&buf[..e.valid_up_to()]).into_owned()),
        Err(_) => None,
    }
}

const MAX_PDF_BYTES: u64 = 20 * 1024 * 1024;
const MAX_PDF_PAGES: usize = 50;
const MAX_PDF_SECONDS: u64 = 5;
const MAX_DOCX_XML_BYTES: u64 = 10 * 1024 * 1024;

/// Text of a file by type: PDF and docx are extracted, anything else is read
/// as plain text. None on any error, over a cap or for binary content.
pub(crate) fn read_document(path: &Path, name: &str) -> Option<String> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let text = match ext.as_str() {
        "pdf" => pdf_text(path),
        "docx" => docx_text(path),
        _ => return read_text(path, MAX_TEXT_BYTES),
    }?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Text of the first `MAX_PDF_PAGES` pages, at most 5 s of extraction and
/// `MAX_TEXT_BYTES` of text. A broken or encrypted PDF (or a parser panic)
/// gives None.
fn pdf_text(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > MAX_PDF_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    std::panic::catch_unwind(|| {
        let doc = lopdf::Document::load_mem(&bytes).ok()?;
        if doc.is_encrypted() {
            return None;
        }
        let started = Instant::now();
        let mut out = String::new();
        for page in doc.get_pages().keys().take(MAX_PDF_PAGES) {
            if started.elapsed() > Duration::from_secs(MAX_PDF_SECONDS) || out.len() as u64 >= MAX_TEXT_BYTES {
                break;
            }
            if let Ok(t) = doc.extract_text(&[*page]) {
                out.push_str(&t);
                out.push('\n');
            }
        }
        out.truncate(floor_boundary(&out, MAX_TEXT_BYTES as usize));
        Some(out)
    })
    .ok()
    .flatten()
}

fn floor_boundary(s: &str, max: usize) -> usize {
    let mut i = max.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Paragraph text of a Word document (`word/document.xml` in the zip).
fn docx_text(path: &Path) -> Option<String> {
    use std::io::Read;
    if std::fs::metadata(path).ok()?.len() > MAX_PDF_BYTES {
        return None;
    }
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
    let mut xml = String::new();
    zip.by_name("word/document.xml").ok()?.take(MAX_DOCX_XML_BYTES).read_to_string(&mut xml).ok()?;
    let mut out = String::new();
    let mut rest = xml.as_str();
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else { break };
        let tag = &rest[lt + 1..lt + gt];
        if tag.starts_with("/w:p") && !tag.starts_with("/w:pPr") || tag.starts_with("w:br") || tag.starts_with("w:tab") {
            out.push('\n');
        }
        rest = &rest[lt + gt + 1..];
    }
    let out = out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    out.truncate(floor_boundary(&out, MAX_TEXT_BYTES as usize));
    Some(out)
}

fn is_image_name(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

fn thumbnail(path: &Path) -> Option<Vec<u8>> {
    if std::fs::metadata(path).ok()?.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let (w, h) = image::image_dimensions(path).ok()?;
    if (w as u64) * (h as u64) > MAX_IMAGE_PIXELS {
        return None;
    }
    let small = image::open(path).ok()?.thumbnail(THUMB_PX, THUMB_PX);
    let mut out = Vec::new();
    small.write_to(&mut std::io::Cursor::new(&mut out), image::ImageOutputFormat::Png).ok()?;
    Some(out)
}

const STOP: &[&str] = &["that", "this", "with", "from", "have", "will", "your", "they", "their", "there", "which", "would", "about", "been", "were", "what"];

/// A short extractive summary: the `max` highest-scoring sentences by word
/// frequency, in their original order. No model, no cost.
pub fn summarize(text: &str, max: usize) -> String {
    let sentences: Vec<&str> = text
        .split(|c| matches!(c, '.' | '!' | '?' | '\n'))
        .map(str::trim)
        .filter(|s| s.chars().count() >= 20)
        .take(500)
        .collect();
    if sentences.len() <= max {
        return sentences.join(". ");
    }
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .map(|w| w.to_lowercase())
            .filter(|w| w.chars().count() >= 4 && !STOP.contains(&w.as_str()))
            .collect()
    };
    let mut freq: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for s in &sentences {
        for w in words(s) {
            *freq.entry(w).or_insert(0.0) += 1.0;
        }
    }
    let mut scored: Vec<(usize, f64)> = sentences
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let ws = words(s);
            let total: f64 = ws.iter().map(|w| freq.get(w).copied().unwrap_or(0.0)).sum();
            (i, total / (ws.len().max(1) as f64).sqrt())
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let mut keep: Vec<usize> = scored.into_iter().take(max).map(|(i, _)| i).collect();
    keep.sort_unstable();
    keep.into_iter().map(|i| sentences[i]).collect::<Vec<_>>().join(". ")
}

/// Groups of live files with the same content hash, biggest saving first.
pub fn dedup(engine_dir: &Path, root_ids: &[String], limit: usize) -> rusqlite::Result<Vec<DedupGroup>> {
    if root_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ec = Connection::open_with_flags(engine_dir.join("index.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    ec.busy_timeout(Duration::from_secs(5))?;
    let marks = (0..root_ids.len()).map(|i| format!("?{}", i + 1)).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT content_hash, size, group_concat(item_id, ','), COUNT(*) FROM items \
         WHERE trash_id IS NULL AND kind = 'file' AND link_escapes = 0 AND parent_id IS NOT NULL \
         AND size > 0 AND content_hash IS NOT NULL AND root_id IN ({marks}) \
         GROUP BY content_hash, size HAVING COUNT(*) > 1 ORDER BY size * (COUNT(*) - 1) DESC, content_hash LIMIT {limit}"
    );
    let mut s = ec.prepare(&sql)?;
    let rows = s.query_map(params_from_iter(root_ids.iter()), |r| {
        let ids: String = r.get(2)?;
        Ok(DedupGroup {
            content_hash: r.get(0)?,
            size: r.get::<_, i64>(1)?.max(0) as u64,
            item_ids: ids.split(',').map(str::to_string).collect(),
        })
    })?;
    rows.collect()
}

// ---- scope ----

/// Whether a principal of this kind may read content of `root` at all.
/// Guests reach files only through a share (step 14's gateway), so a guest
/// gets nothing from these routes. Agents only see roots that allow agent
/// processing.
pub(crate) fn kind_may_read(kind: PrincipalKind, root: &RootRecord) -> bool {
    match kind {
        PrincipalKind::Owner | PrincipalKind::OwnDevice => root.policy.read,
        PrincipalKind::Agent => root.policy.read && root.policy.agent_process,
        PrincipalKind::Guest => false,
    }
}

/// Roots this caller may search now (kind rules, then the protected-root unlock).
pub(crate) fn scoped_roots(v: &Inner, req: &Req, ids: Option<&[String]>) -> Result<Vec<RootRecord>, Fail> {
    Ok(v.drive
        .roots()?
        .into_iter()
        .filter(|r| ids.map_or(true, |ids| ids.contains(&r.root_id)))
        .filter(|r| kind_may_read(req.principal.kind, r))
        .filter(|r| v.guard(req, &r.root_id, Need::Read).is_ok())
        .collect())
}

/// FTS5 expression for a query. `"quoted words"` match as a phrase; anything
/// else matches items with any of the words, best match first.
pub(crate) fn match_expr(q: &str) -> Option<String> {
    let t = q.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let words: Vec<String> =
            t[1..t.len() - 1].split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect();
        return (!words.is_empty()).then(|| format!("\"{}\"", words.join(" ")));
    }
    crate::memory_index::fts_query(t)
}

/// `full_text` and `semantic` search for the `POST /search` route.
pub(crate) fn search_content(
    v: &Inner,
    req: &Req,
    roots: &[RootRecord],
    body: &SearchRequest,
    query: &str,
    limit: usize,
    start: usize,
) -> Result<SearchResponse, Fail> {
    let ids: Vec<String> = roots.iter().map(|r| r.root_id.clone()).collect();
    let intel = &v.intel;
    // Folders have no content to match.
    let found: Vec<TextHit> = if body.kind == Some(ItemKind::Folder) {
        Vec::new()
    } else if body.mode == SearchMode::Semantic {
        let e = intel.embedder().embed(&[query.to_string()], true).ok_or_else(|| {
            ApiError::unsupported("Meaning search needs the local embedding model, which isn't running on this computer. Full text search still works.")
        })?;
        let qv = e.vectors.into_iter().next().map(normalized).ok_or_else(ApiError::internal)?;
        intel.search_semantic(&qv, &e.model, &ids, body.modified_after.as_deref(), limit + 1, start)?
    } else {
        let expr = match_expr(query).ok_or_else(|| ApiError::invalid("Search for words, not only punctuation."))?;
        intel.search_full_text(&expr, &ids, body.modified_after.as_deref(), limit + 1, start)?
    };
    let next = (found.len() > limit).then(|| format!("s1.{}", start + limit));
    let mut hits = Vec::new();
    for hit in found.into_iter().take(limit) {
        let Ok(rec) = v.drive.stat(&hit.item_id) else { continue };
        let Some(root) = roots.iter().find(|r| r.root_id == rec.root_id) else { continue };
        if rec.link_escapes {
            continue;
        }
        // Snippets are file content: never for a guest (SearchHit schema note).
        let snippet = if req.principal.kind == PrincipalKind::Guest { None } else { hit.snippet };
        hits.push(SearchHit { item: v.item(root, &rec), score: hit.score, snippet });
    }
    let state = if intel.caught_up() { SearchResponseIndexState::Ready } else { SearchResponseIndexState::Building };
    Ok(SearchResponse { hits, next_page_token: next, index_state: state, freshness: super::live() })
}

// ---- the background pass ----

pub struct PassReport {
    pub indexed: usize,
    pub forgot: usize,
    /// A full batch was found: more may be waiting.
    pub more: bool,
}

pub(crate) fn now_string(v: &Inner) -> String {
    v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Indexes one batch. After each file `pace(busy, rest)` is called with the
/// time the file took and the rest the budget requires; the worker sleeps for
/// `rest`, a test records it.
pub(crate) fn run_pass(v: &Inner, pace: &mut dyn FnMut(Duration, Duration)) -> Result<PassReport, Fail> {
    let intel = &v.intel;
    let budget = intel.budget();
    let roots = v.drive.roots()?;
    // Protected roots (Memory Drive) only when the owner opted them in.
    let mut ids = Vec::new();
    for r in roots.iter().filter(|r| r.policy.read) {
        if !r.policy.protected || intel.opted_in(&r.root_id)? {
            ids.push(r.root_id.clone());
        }
    }
    let forgot = intel.forget_outside(&ids)? + intel.forget_gone(v)?;
    let cands = intel.pending(&v.cfg.data_dir.join("engine"), &ids, budget.batch_files)?;
    let now = now_string(v);
    let mut indexed = 0;
    for cand in &cands {
        let started = Instant::now();
        let done = match v.drive.local_path(&cand.item_id) {
            Ok(path) => intel.index_one(cand, &path, &now).ok(),
            Err(_) => None,
        };
        match done {
            Some(text) => {
                indexed += 1;
                if let Some(text) = text {
                    apply_rules(v, cand, &text);
                }
            }
            None => {
                let _ = intel.mark_skipped(cand, &now);
            }
        }
        let busy = started.elapsed();
        pace(busy, budget.rest_after(busy));
    }
    intel.caught_up.store(cands.is_empty(), Ordering::Relaxed);
    Ok(PassReport { indexed, forgot, more: cands.len() >= budget.batch_files })
}

/// Folder rules the owner made: a new or changed file under a rule's folder
/// gets the rule's action. Only `summarize` exists.
fn apply_rules(v: &Inner, cand: &Candidate, text: &str) {
    let Ok(rules) = v.intel.rules() else { return };
    if rules.is_empty() {
        return;
    }
    let mut ancestors = HashSet::new();
    let mut at = v.drive.stat(&cand.item_id).ok().and_then(|r| r.parent_id);
    while let Some(id) = at {
        if !ancestors.insert(id.clone()) || ancestors.len() > 64 {
            break;
        }
        at = v.drive.stat(&id).ok().and_then(|r| r.parent_id);
    }
    if rules.iter().any(|(_, folder, action)| action == "summarize" && ancestors.contains(folder)) {
        let _ = v.intel.store_summary(&cand.item_id, &cand.content_hash, &summarize(text, 3), &now_string(v));
    }
}

/// Starts the one low-priority worker. It holds only a weak reference, so it
/// ends when the drive is dropped.
pub(crate) fn spawn_worker(inner: &Arc<Inner>) {
    let weak: Weak<Inner> = Arc::downgrade(inner);
    let spawned = std::thread::Builder::new().name("files-intel".into()).spawn(move || {
        lower_priority();
        let mut backoff = Backoff::new();
        loop {
            let Some(v) = weak.upgrade() else { return };
            let wait = match run_pass(&v, &mut |_busy, rest| std::thread::sleep(rest)) {
                Ok(r) if r.more => {
                    backoff.reset();
                    Duration::from_millis(250)
                }
                Ok(r) if r.indexed > 0 || r.forgot > 0 => {
                    backoff.reset();
                    backoff.next_wait()
                }
                _ => backoff.next_wait(),
            };
            drop(v);
            std::thread::sleep(wait);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "files v2: the index worker did not start; content search stays empty");
    }
}

/// Linux treats `setpriority(PRIO_PROCESS, 0)` as the calling thread; macOS
/// would change the whole process, so it is skipped there (the duty cycle
/// still caps use).
fn lower_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}

/// `ALLTERNIT_FILES_INTEL=off` turns the worker off.
pub(crate) fn worker_enabled() -> bool {
    !std::env::var("ALLTERNIT_FILES_INTEL").map(|v| v.trim().eq_ignore_ascii_case("off") || v.trim() == "0").unwrap_or(false)
}
